use std::sync::LazyLock;

use clap::Parser;

mod sqs;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
  /// Queue URL to fetch/purge messages
  #[arg(short = 'q', long)]
  queue: String,

  /// AWS Region for SQS
  #[arg(short, long, default_value_t = "ap-south-1".to_string())]
  region: String,

  /// File name to store the data
  #[arg(short, long = "fileName", default_value_t = "queue_messages.json".to_string())]
  file_name: String,

  /// AWS Profile to access account
  #[arg(short, long, default_value_t = "DEFAULT".to_string())]
  profile: String,

  /// Purge messages in queue
  #[arg(short, long)]
  delete: bool,

  /// Enable verbose logging
  #[arg(short, long)]
  verbose: bool,
}

pub(crate) static ARGS: LazyLock<Args> = LazyLock::new(Args::parse);

#[tokio::main]
async fn main() {
  sqs::execute().await;
}
