use std::num::NonZeroUsize;
use std::process::ExitCode;
use std::sync::LazyLock;
use std::thread;

use clap::Parser;

mod output;
mod progress;
mod sqs;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
  /// Queue URL to fetch/purge messages
  #[arg(short = 'q', long)]
  queue: String,

  /// AWS Region for SQS
  #[arg(short, long, default_value = "ap-south-1")]
  region: String,

  /// File name to store the data
  #[arg(short, long = "fileName", default_value = "queue_messages.json")]
  file_name: String,

  /// AWS Profile to access account (defaults to the AWS SDK default profile)
  #[arg(short, long)]
  profile: Option<String>,

  /// Purge messages in queue
  #[arg(short, long)]
  delete: bool,

  /// Enable verbose logging
  #[arg(short, long)]
  verbose: bool,

  /// Number of concurrent pollers (defaults to the number of CPUs)
  #[arg(long, default_value_t = default_pollers())]
  pollers: NonZeroUsize,

  /// Seconds received messages stay hidden from other consumers while fetching (defaults to the queue setting).
  /// Fetching can stop early if this expires before the whole queue is read.
  #[arg(long, value_parser = clap::value_parser!(i32).range(0..=43200))]
  visibility_timeout: Option<i32>,
}

fn default_pollers() -> NonZeroUsize {
  thread::available_parallelism().unwrap_or(NonZeroUsize::MIN)
}

pub(crate) static ARGS: LazyLock<Args> = LazyLock::new(Args::parse);

#[tokio::main]
async fn main() -> ExitCode {
  match sqs::execute().await {
    Ok(()) => ExitCode::SUCCESS,
    Err(err) => {
      eprintln!("{err:#}");
      ExitCode::FAILURE
    }
  }
}
