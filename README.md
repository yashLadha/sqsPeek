<div align="center">

<img src="assets/logo.svg" alt="sqsPeek logo" width="128" height="128">

# sqsPeek

**Dump every message in an Amazon SQS queue to a JSON file, and optionally purge it.**

[![Head Release](https://github.com/yashLadha/sqsPeek/actions/workflows/head-release.yaml/badge.svg)](https://github.com/yashLadha/sqsPeek/actions/workflows/head-release.yaml)
[![Latest release](https://img.shields.io/github/v/release/yashLadha/sqsPeek?color=e7157b)](https://github.com/yashLadha/sqsPeek/releases/latest)
![Rust 2024](https://img.shields.io/badge/rust-2024_edition-8c1bd6?logo=rust)

[Install](#install) · [Usage](#usage) · [Options](#options) · [How it works](#how-it-works) · [Development](#development)

</div>

---

## Why

The AWS console is fine for inspecting ten messages in a dead-letter queue. With thousands, pagination makes it painful, and there is no way to export them for analysis. `sqsPeek` reads the whole queue in one command and writes it to a file you can feed to `jq` or any other tool.

## Features

- **Complete reads**: concurrent long-polling workers keep receiving until the queue is drained, and redeliveries are deduplicated by message ID.
- **Full fidelity**: bodies, system attributes and message attributes are all captured; binary attributes are base64 encoded.
- **Safe purging**: `--delete` only removes messages after every one of them has been written to disk. If fetching fails or is interrupted, messages are released back to the queue instead.
- **No clobbered output**: results are streamed to `<file>.partial` and moved into place only on success.
- **Graceful Ctrl+C**: the first interrupt finishes safely, the second exits immediately.
- **Portable binaries**: statically linked builds for Linux, macOS and Windows.

## Install

Download a binary for your platform from the [latest release](https://github.com/yashLadha/sqsPeek/releases/latest), or the `head` prerelease for the newest build of `main`.

Or build from source with Cargo:

```shell
cargo install --git https://github.com/yashLadha/sqsPeek
```

## Usage

Credentials are resolved through the standard AWS SDK chain (environment, profile, SSO, instance role).

**Dump a queue to disk**

```shell
sqsPeek -q https://sqs.us-east-1.amazonaws.com/123456789012/orders-dlq -r us-east-1
```

Messages are written to `queue_messages.json` and become visible on the queue again once the run finishes.

**Dump, then purge**

```shell
sqsPeek -q "$QUEUE_URL" -r us-east-1 -d -f orders-dlq.json
```

**Use a named profile**

```shell
sqsPeek -q "$QUEUE_URL" -r eu-west-1 -p staging
```

**Inspect the output**

```shell
jq '.[].Body' queue_messages.json
```

Each entry looks like:

```json
{
  "MessageId": "8d1c4f6e-...",
  "ReceiptHandle": "AQEB...",
  "MD5OfBody": "5d41402abc4b2a76b9719d911017c592",
  "Body": "{\"orderId\": 42}",
  "Attributes": {
    "ApproximateReceiveCount": "3",
    "SentTimestamp": "1700000000000"
  },
  "MD5OfMessageAttributes": null,
  "MessageAttributes": {}
}
```

## Options

| Flag | Default | Description |
|---|---|---|
| `-q`, `--queue <URL>` | required | Queue URL to fetch or purge |
| `-r`, `--region <REGION>` | `ap-south-1` | AWS region of the queue |
| `-p`, `--profile <NAME>` | SDK default | AWS profile to use |
| `-f`, `--fileName <PATH>` | `queue_messages.json` | Output file |
| `-d`, `--delete` | off | Purge messages after they are saved |
| `--pollers <N>` | CPU count | Number of concurrent pollers |
| `--visibility-timeout <SECS>` | queue setting | How long received messages stay hidden while fetching (0 to 43200) |
| `-v`, `--verbose` | off | Timestamped debug logging instead of the progress counter |
| `-h`, `--help` | | Show help |
| `-V`, `--version` | | Show version |

## How it works

SQS has no read-only peek, so `sqsPeek` receives messages, which hides them from other consumers for the visibility timeout. Once the queue is drained it either deletes them (`--delete`) or resets their visibility to zero so they are immediately available again.

If the visibility timeout expires before the whole queue has been read, messages reappear and the run may stop early. For large queues, raise `--visibility-timeout` so it covers the full fetch.

> [!WARNING]
> While `sqsPeek` runs, fetched messages are invisible to other consumers of the queue. Avoid running it against a queue that live workers depend on.

## Development

```shell
cargo build --release
cargo test
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

Pushes to `main` publish a `head` prerelease; creating a GitHub release builds and attaches binaries for every supported target.
