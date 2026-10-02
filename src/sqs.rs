use std::collections::{BTreeMap, HashMap};
use std::process;
use std::sync::{Arc, Mutex};

use anyhow::{Context, anyhow, bail};
use aws_config::Region;
use aws_sdk_sqs::Client;
use aws_sdk_sqs::types::{
  BatchResultErrorEntry, ChangeMessageVisibilityBatchRequestEntry, DeleteMessageBatchRequestEntry,
  Message, MessageSystemAttributeName,
};
use tokio::signal;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;

use crate::ARGS;
use crate::output::{JsonArrayWriter, PeekedMessage};
use crate::progress::{self, Progress};

macro_rules! debug_log {
  ($($arg:tt)*) => {
    if $crate::ARGS.verbose {
      eprintln!(
        "{} {}",
        chrono::Local::now().format("%Y/%m/%d %H:%M:%S"),
        format_args!($($arg)*)
      );
    }
  };
}

// Maximum number of messages a single ReceiveMessage or batch call handles.
const MAX_MESSAGES: i32 = 10;

// Capacity of the channel between the pollers and the file writer.
const BUFFER: usize = 100;

// Long polling queries every SQS server, so unlike short polling an empty
// response means the queue really had nothing available.
const WAIT_TIME_SECONDS: i32 = 1;

// Receives in a row without any new message before a poller treats the queue as drained.
const IDLE_RECEIVES_TO_STOP: u32 = 2;

// (message ID, receipt handle)
type Handle = (String, String);

#[derive(Default)]
struct Receipts {
  handles: HashMap<String, String>,
  duplicates: usize,
}

impl Receipts {
  /// Records the newest receipt handle of every message and returns only the
  /// ones not seen before, since a message reappears once its visibility
  /// timeout expires.
  fn keep_new(&mut self, messages: Vec<Message>) -> Vec<Message> {
    messages
      .into_iter()
      .filter(|message| {
        let (Some(message_id), Some(receipt_handle)) =
          (&message.message_id, &message.receipt_handle)
        else {
          return false;
        };
        let is_new = self.handles.insert(message_id.clone(), receipt_handle.clone()).is_none();
        if !is_new {
          self.duplicates += 1;
        }
        is_new
      })
      .collect()
  }
}

#[derive(Clone, Copy)]
enum BatchAction {
  Delete,
  Release,
}

impl BatchAction {
  fn verb(self) -> &'static str {
    match self {
      BatchAction::Delete => "delete",
      BatchAction::Release => "release",
    }
  }

  fn label(self) -> &'static str {
    match self {
      BatchAction::Delete => "Purged",
      BatchAction::Release => "Released",
    }
  }
}

struct BatchResult {
  attempted: usize,
  failed: usize,
  errors: Vec<String>,
}

async fn client() -> Client {
  debug_log!(
    "Creating AWS session (region={}, profile={})",
    ARGS.region,
    ARGS.profile.as_deref().unwrap_or("SDK default")
  );
  let mut loader = aws_config::from_env().region(Region::new(ARGS.region.as_str()));
  if let Some(profile) = &ARGS.profile {
    loader = loader.profile_name(profile);
  }
  Client::new(&loader.load().await)
}

/// Resolves on the first Ctrl+C; a second one exits immediately.
fn watch_interrupts() -> oneshot::Receiver<()> {
  let (tx, rx) = oneshot::channel();
  tokio::spawn(async move {
    if signal::ctrl_c().await.is_err() {
      return;
    }
    eprintln!("\nInterrupted, finishing safely. Press Ctrl+C again to exit immediately");
    let _ = tx.send(());
    if signal::ctrl_c().await.is_ok() {
      process::exit(130);
    }
  });
  rx
}

async fn receive(client: &Client) -> anyhow::Result<Vec<Message>> {
  let output = client
    .receive_message()
    .queue_url(&ARGS.queue)
    .max_number_of_messages(MAX_MESSAGES)
    .wait_time_seconds(WAIT_TIME_SECONDS)
    .set_visibility_timeout(ARGS.visibility_timeout)
    .message_system_attribute_names(MessageSystemAttributeName::All)
    .message_attribute_names("All")
    .send()
    .await
    .context("Failed to receive messages")?;
  Ok(output.messages.unwrap_or_default())
}

async fn poll(
  id: usize,
  client: Client,
  receipts: Arc<Mutex<Receipts>>,
  tx: mpsc::Sender<Message>,
) -> anyhow::Result<()> {
  let mut total = 0;
  let mut idle_receives = 0;
  while idle_receives < IDLE_RECEIVES_TO_STOP {
    debug_log!("Poller {id} polling for messages (total so far {total})");
    let received = receive(&client).await?;
    let messages = receipts.lock().expect("receipts lock poisoned").keep_new(received);
    if messages.is_empty() {
      idle_receives += 1;
      continue;
    }
    idle_receives = 0;
    total += messages.len();
    debug_log!("Poller {id} polled {} new messages (total so far {total})", messages.len());

    for message in messages {
      if tx.send(message).await.is_err() {
        return Ok(());
      }
    }
  }
  debug_log!("Poller {id} finished with {total} messages");
  Ok(())
}

fn write_error() -> String {
  format!("Failed to write {}", ARGS.file_name)
}

async fn write_messages(mut rx: mpsc::Receiver<Message>) -> anyhow::Result<usize> {
  debug_log!("Writing output to {}", ARGS.file_name);
  let mut writer = JsonArrayWriter::create(&ARGS.file_name).with_context(write_error)?;
  let mut progress = Progress::new("Fetched", None, progress::enabled(ARGS.verbose));
  while let Some(message) = rx.recv().await {
    writer.push(&PeekedMessage::from(message)).with_context(write_error)?;
    progress.add(1);
  }
  progress.finish();
  writer.finish().with_context(write_error)
}

/// Runs the pollers and writes their messages to the output file. Returns every
/// received receipt handle, plus whether fetching failed or was interrupted.
async fn fetch(
  client: &Client,
  pollers: usize,
  interrupted: oneshot::Receiver<()>,
) -> (Receipts, bool) {
  debug_log!("Starting {pollers} pollers");
  let receipts = Arc::new(Mutex::new(Receipts::default()));
  let (tx, rx) = mpsc::channel(BUFFER);
  let mut tasks = JoinSet::new();
  for id in 0..pollers {
    tasks.spawn(poll(id, client.clone(), receipts.clone(), tx.clone()));
  }
  // The receiver only finishes once every sender is dropped, including this one.
  drop(tx);

  // Dropping the writer on interrupt closes the channel, which stops the pollers.
  let written = tokio::select! {
    written = write_messages(rx) => written,
    Ok(()) = interrupted => Err(anyhow!("Interrupted before fetching finished")),
  };
  let mut failed = false;
  match written {
    Ok(written) => println!("Fetched {written} records"),
    Err(err) => {
      eprintln!("{err:#}");
      failed = true;
    }
  }

  // Pollers usually fail for the same reason, so report each distinct error once.
  let mut poll_errors = BTreeMap::<String, usize>::new();
  while let Some(result) = tasks.join_next().await {
    if let Err(err) = result.expect("poller task panicked") {
      *poll_errors.entry(format!("{err:#}")).or_default() += 1;
    }
  }
  for (err, count) in &poll_errors {
    eprintln!("{err} ({count} of {pollers} pollers)");
  }
  failed |= !poll_errors.is_empty();

  let receipts = std::mem::take(&mut *receipts.lock().expect("receipts lock poisoned"));
  (receipts, failed)
}

async fn send_batch(
  client: &Client,
  action: BatchAction,
  batch: &[Handle],
) -> anyhow::Result<Vec<BatchResultErrorEntry>> {
  // Entry IDs only need to be unique within a batch.
  let entries = batch.iter().enumerate().map(|(idx, (_, handle))| (idx.to_string(), handle));
  let failures = match action {
    BatchAction::Delete => {
      let entries = entries
        .map(|(id, handle)| DeleteMessageBatchRequestEntry::builder().id(id).receipt_handle(handle))
        .map(|entry| entry.build())
        .collect::<Result<_, _>>()?;
      let request = client.delete_message_batch().queue_url(&ARGS.queue);
      request.set_entries(Some(entries)).send().await?.failed
    }
    BatchAction::Release => {
      let entries = entries
        .map(|(id, handle)| {
          ChangeMessageVisibilityBatchRequestEntry::builder().id(id).receipt_handle(handle)
        })
        .map(|entry| entry.visibility_timeout(0).build())
        .collect::<Result<_, _>>()?;
      let request = client.change_message_visibility_batch().queue_url(&ARGS.queue);
      request.set_entries(Some(entries)).send().await?.failed
    }
  };
  Ok(failures)
}

fn describe_failure(
  action: BatchAction,
  batch: &[Handle],
  failure: &BatchResultErrorEntry,
) -> String {
  let handle = failure.id().parse::<usize>().ok().and_then(|idx| batch.get(idx));
  let message_id = handle.map_or("unknown", |(message_id, _)| message_id.as_str());
  let reason = failure.message().unwrap_or(failure.code());
  format!("Failed to {} message {message_id}: {reason}", action.verb())
}

async fn run_batch(client: Client, action: BatchAction, batch: Vec<Handle>) -> BatchResult {
  let attempted = batch.len();
  let sent = send_batch(&client, action, &batch).await;
  match sent.with_context(|| format!("Batch {} failed", action.verb())) {
    Ok(failures) => BatchResult {
      attempted,
      failed: failures.len(),
      errors: failures.iter().map(|failure| describe_failure(action, &batch, failure)).collect(),
    },
    Err(err) => BatchResult { attempted, failed: attempted, errors: vec![format!("{err:#}")] },
  }
}

/// Applies `action` to every handle with at most `workers` batches in flight,
/// returning how many messages it failed for.
async fn run_batches(
  client: &Client,
  action: BatchAction,
  handles: HashMap<String, String>,
  workers: usize,
) -> usize {
  let mut progress =
    Progress::new(action.label(), Some(handles.len()), progress::enabled(ARGS.verbose));
  let mut handles = handles.into_iter();
  let mut batches = std::iter::from_fn(move || {
    let batch: Vec<Handle> = handles.by_ref().take(MAX_MESSAGES as usize).collect();
    (!batch.is_empty()).then_some(batch)
  });

  let mut tasks = JoinSet::new();
  for batch in batches.by_ref().take(workers) {
    tasks.spawn(run_batch(client.clone(), action, batch));
  }
  let mut failed = 0;
  while let Some(result) = tasks.join_next().await {
    let result = result.expect("batch task panicked");
    for err in &result.errors {
      progress.println(err);
    }
    failed += result.failed;
    progress.add(result.attempted);
    if let Some(batch) = batches.next() {
      tasks.spawn(run_batch(client.clone(), action, batch));
    }
  }
  progress.finish();
  failed
}

/// Purges the fetched messages when that is safe, otherwise makes them visible
/// again. Returns whether any of them failed.
async fn settle(client: &Client, handles: HashMap<String, String>, fetch_failed: bool) -> bool {
  // Never purge after a partial fetch: the file may not hold every message.
  let action = if ARGS.delete && !fetch_failed {
    BatchAction::Delete
  } else {
    if ARGS.delete {
      eprintln!(
        "Not purging because fetching did not complete; releasing fetched messages instead"
      );
    }
    BatchAction::Release
  };
  let total = handles.len();
  let failed = run_batches(client, action, handles, ARGS.pollers.get()).await;
  match action {
    BatchAction::Delete => println!("Purged {} records", total - failed),
    BatchAction::Release => {
      debug_log!("Released {} records back to the queue", total - failed);
    }
  }
  failed > 0
}

pub async fn execute() -> anyhow::Result<()> {
  let interrupted = watch_interrupts();
  let client = client().await;
  debug_log!("SQS Client initialized for queue {}", ARGS.queue);

  let (receipts, fetch_failed) = fetch(&client, ARGS.pollers.get(), interrupted).await;
  if receipts.duplicates > 0 {
    debug_log!("Skipped {} duplicate deliveries", receipts.duplicates);
  }
  let settle_failed = settle(&client, receipts.handles, fetch_failed).await;

  if fetch_failed || settle_failed {
    bail!("Finished with errors");
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn message(id: &str, receipt_handle: &str) -> Message {
    Message::builder().message_id(id).receipt_handle(receipt_handle).build()
  }

  #[test]
  fn keep_new_drops_redeliveries_and_keeps_newest_handle() {
    let mut receipts = Receipts::default();

    let first = receipts.keep_new(vec![message("a", "a1"), message("b", "b1")]);
    let second = receipts.keep_new(vec![message("a", "a2"), message("c", "c1")]);

    let ids = |messages: &[Message]| -> Vec<_> {
      messages.iter().filter_map(|message| message.message_id.clone()).collect()
    };
    assert_eq!(ids(&first), ["a", "b"]);
    assert_eq!(ids(&second), ["c"]);
    assert_eq!(receipts.duplicates, 1);
    assert_eq!(receipts.handles["a"], "a2");
    assert_eq!(receipts.handles.len(), 3);
  }

  #[test]
  fn keep_new_skips_messages_without_id_or_handle() {
    let mut receipts = Receipts::default();
    let incomplete = Message::builder().message_id("a").build();

    assert!(receipts.keep_new(vec![incomplete]).is_empty());
    assert!(receipts.handles.is_empty());
  }
}
