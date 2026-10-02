use std::collections::BTreeMap;
use std::process;
use std::thread;

use aws_config::Region;
use aws_sdk_sqs::Client;
use aws_sdk_sqs::error::DisplayErrorContext;
use aws_sdk_sqs::types::{
  DeleteMessageBatchRequestEntry, Message, MessageAttributeValue, MessageSystemAttributeName,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Serialize;
use tokio::sync::{OnceCell, mpsc};
use tokio::task::JoinSet;

use crate::ARGS;

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

// Maximum number of messages a single ReceiveMessage or DeleteMessageBatch call handles.
const MAX_MESSAGES: i32 = 10;

static CLIENT: OnceCell<Client> = OnceCell::const_new();

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct PeekedMessage {
  message_id: Option<String>,
  receipt_handle: Option<String>,
  #[serde(rename = "MD5OfBody")]
  md5_of_body: Option<String>,
  body: Option<String>,
  attributes: BTreeMap<String, String>,
  #[serde(rename = "MD5OfMessageAttributes")]
  md5_of_message_attributes: Option<String>,
  message_attributes: BTreeMap<String, PeekedAttribute>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct PeekedAttribute {
  data_type: String,
  string_value: Option<String>,
  binary_value: Option<String>,
}

impl From<Message> for PeekedMessage {
  fn from(message: Message) -> Self {
    PeekedMessage {
      message_id: message.message_id,
      receipt_handle: message.receipt_handle,
      md5_of_body: message.md5_of_body,
      body: message.body,
      attributes: message
        .attributes
        .unwrap_or_default()
        .into_iter()
        .map(|(name, value)| (name.as_str().to_string(), value))
        .collect(),
      md5_of_message_attributes: message.md5_of_message_attributes,
      message_attributes: message
        .message_attributes
        .unwrap_or_default()
        .into_iter()
        .map(|(name, value)| (name, value.into()))
        .collect(),
    }
  }
}

impl From<MessageAttributeValue> for PeekedAttribute {
  fn from(value: MessageAttributeValue) -> Self {
    PeekedAttribute {
      data_type: value.data_type,
      string_value: value.string_value,
      binary_value: value
        .binary_value
        .map(|blob| STANDARD.encode(blob.as_ref())),
    }
  }
}

async fn client() -> &'static Client {
  CLIENT
    .get_or_init(|| async {
      debug_log!(
        "Creating AWS session (region={}, profile={})",
        ARGS.region,
        ARGS.profile
      );
      let session = aws_config::from_env()
        .profile_name(&ARGS.profile)
        .region(Region::new(ARGS.region.as_str()))
        .load()
        .await;
      Client::new(&session)
    })
    .await
}

async fn poll(id: usize, client: &'static Client, tx: mpsc::Sender<Message>) {
  let mut total = 0;
  loop {
    debug_log!("Poller {id} polling for messages (total so far {total})");
    let output = client
      .receive_message()
      .queue_url(&ARGS.queue)
      .max_number_of_messages(MAX_MESSAGES)
      .wait_time_seconds(0)
      .message_system_attribute_names(MessageSystemAttributeName::All)
      .message_attribute_names("All")
      .send()
      .await
      .unwrap_or_else(|err| {
        eprintln!(
          "Got error in receiving message: {}",
          DisplayErrorContext(&err)
        );
        process::exit(1);
      });

    let messages = output.messages.unwrap_or_default();
    if messages.is_empty() {
      break;
    }
    total += messages.len();
    debug_log!(
      "Poller {id} polled {} messages (total so far {total})",
      messages.len()
    );

    for message in messages {
      if tx.send(message).await.is_err() {
        return;
      }
    }
  }
  debug_log!("Poller {id} finished with {total} messages");
}

fn write_file(messages: &[PeekedMessage]) {
  println!("Fetched {} records", messages.len());
  let json = serde_json::to_vec_pretty(messages).unwrap_or_else(|err| {
    eprintln!("Error in marshalling data: {err}");
    process::exit(1);
  });
  debug_log!("Writing output to {}", ARGS.file_name);
  std::fs::write(&ARGS.file_name, json).unwrap_or_else(|err| {
    eprintln!("Error in writing to file: {err}");
    process::exit(1);
  });
}

async fn delete_batch(client: &'static Client, batch: Vec<(Option<String>, String)>) -> usize {
  // Entry IDs only need to be unique within a batch, and message IDs are not
  // guaranteed to be when a message is received more than once.
  let entries = batch
    .iter()
    .enumerate()
    .map(|(idx, (_, receipt_handle))| {
      DeleteMessageBatchRequestEntry::builder()
        .id(idx.to_string())
        .receipt_handle(receipt_handle)
        .build()
        .expect("id and receipt handle are set")
    })
    .collect();

  let output = client
    .delete_message_batch()
    .queue_url(&ARGS.queue)
    .set_entries(Some(entries))
    .send()
    .await
    .unwrap_or_else(|err| {
      eprintln!(
        "Error received in batch delete: {}",
        DisplayErrorContext(&err)
      );
      process::exit(1);
    });

  for failure in output.failed() {
    let message_id = failure
      .id()
      .parse::<usize>()
      .ok()
      .and_then(|idx| batch.get(idx))
      .and_then(|(message_id, _)| message_id.as_deref())
      .unwrap_or("unknown");
    eprintln!(
      "Failed to delete message {message_id}: {}",
      failure.message().unwrap_or(failure.code())
    );
  }
  output.successful().len()
}

async fn delete_messages(client: &'static Client, messages: &[PeekedMessage], workers: usize) {
  debug_log!("Starting purge of {} messages", messages.len());
  let handles: Vec<_> = messages
    .iter()
    .filter_map(|message| {
      let receipt_handle = message.receipt_handle.clone()?;
      Some((message.message_id.clone(), receipt_handle))
    })
    .collect();
  let mut batches = handles.chunks(MAX_MESSAGES as usize).map(<[_]>::to_vec);

  let mut tasks = JoinSet::new();
  for batch in batches.by_ref().take(workers) {
    tasks.spawn(delete_batch(client, batch));
  }
  let mut purged = 0;
  while let Some(result) = tasks.join_next().await {
    purged += result.expect("delete task panicked");
    if let Some(batch) = batches.next() {
      tasks.spawn(delete_batch(client, batch));
    }
  }
  println!("Purged {purged} records");
}

pub async fn execute() {
  let client = client().await;
  debug_log!("SQS Client initialized for queue {}", ARGS.queue);

  let pollers = thread::available_parallelism().map_or(1, |n| n.get());
  debug_log!("Starting {pollers} pollers");

  let (tx, mut rx) = mpsc::channel(100);
  for id in 0..pollers {
    tokio::spawn(poll(id, client, tx.clone()));
  }
  // The receiver only finishes once every sender is dropped, including this one.
  drop(tx);

  let mut messages = Vec::new();
  while let Some(message) = rx.recv().await {
    messages.push(PeekedMessage::from(message));
  }

  write_file(&messages);

  if ARGS.delete {
    delete_messages(client, &messages, pollers).await;
  }
}
