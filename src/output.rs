use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use aws_sdk_sqs::types::{Message, MessageAttributeValue};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PeekedMessage {
  pub message_id: Option<String>,
  pub receipt_handle: Option<String>,
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
      binary_value: value.binary_value.map(|blob| STANDARD.encode(blob.as_ref())),
    }
  }
}

/// Streams items as a JSON array whose bytes match `serde_json::to_string_pretty`
/// of the equivalent `Vec`, without a trailing newline.
///
/// Items go to `<path>.partial`, which only replaces `path` once `finish`
/// succeeds, so a failed run never clobbers an earlier complete file.
pub struct JsonArrayWriter {
  writer: BufWriter<File>,
  count: usize,
  path: PathBuf,
  partial_path: PathBuf,
  committed: bool,
}

impl JsonArrayWriter {
  pub fn create(path: &str) -> io::Result<Self> {
    let partial_path = PathBuf::from(format!("{path}.partial"));
    Ok(JsonArrayWriter {
      writer: BufWriter::new(File::create(&partial_path)?),
      count: 0,
      path: PathBuf::from(path),
      partial_path,
      committed: false,
    })
  }

  pub fn push<T: Serialize>(&mut self, item: &T) -> io::Result<()> {
    let json = serde_json::to_string_pretty(item)?;
    self.writer.write_all(if self.count == 0 { b"[\n" } else { b",\n" })?;
    for (idx, line) in json.split('\n').enumerate() {
      if idx > 0 {
        self.writer.write_all(b"\n")?;
      }
      self.writer.write_all(b"  ")?;
      self.writer.write_all(line.as_bytes())?;
    }
    self.count += 1;
    Ok(())
  }

  /// Completes the array, moves it into place and returns the number of items written.
  pub fn finish(mut self) -> io::Result<usize> {
    self.writer.write_all(if self.count == 0 { b"[]" } else { b"\n]" })?;
    self.writer.flush()?;
    self.writer.get_ref().sync_all()?;
    fs::rename(&self.partial_path, &self.path)?;
    self.committed = true;
    Ok(self.count)
  }
}

impl Drop for JsonArrayWriter {
  fn drop(&mut self) {
    if !self.committed {
      let _ = fs::remove_file(&self.partial_path);
    }
  }
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;
  use std::sync::atomic::{AtomicUsize, Ordering};

  use aws_sdk_sqs::primitives::Blob;
  use aws_sdk_sqs::types::MessageSystemAttributeName;
  use serde_json::json;

  use super::*;

  fn temp_path() -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
      "sqsPeek-output-test-{}-{}.json",
      std::process::id(),
      COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
  }

  fn write_all<T: Serialize>(items: &[T]) -> String {
    let path = temp_path();
    let mut writer = JsonArrayWriter::create(path.to_str().unwrap()).unwrap();
    for item in items {
      writer.push(item).unwrap();
    }
    assert_eq!(writer.finish().unwrap(), items.len());
    assert!(!partial(&path).exists());
    let written = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    written
  }

  fn partial(path: &std::path::Path) -> PathBuf {
    PathBuf::from(format!("{}.partial", path.display()))
  }

  #[test]
  fn unfinished_writer_keeps_previous_file() {
    let path = temp_path();
    std::fs::write(&path, "previous").unwrap();

    let mut writer = JsonArrayWriter::create(path.to_str().unwrap()).unwrap();
    writer.push(&sample_message("a")).unwrap();
    assert!(partial(&path).exists());
    drop(writer);

    assert!(!partial(&path).exists());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "previous");
    std::fs::remove_file(&path).unwrap();
  }

  fn sample_message(id: &str) -> PeekedMessage {
    Message::builder()
      .message_id(id)
      .receipt_handle(format!("handle-{id}"))
      .body("line one\nline two \"quoted\"")
      .attributes(MessageSystemAttributeName::SentTimestamp, "1700000000000")
      .message_attributes(
        "Kind",
        MessageAttributeValue::builder().data_type("String").string_value("test").build().unwrap(),
      )
      .build()
      .into()
  }

  #[test]
  fn writer_matches_pretty_for_zero_items() {
    let items: Vec<PeekedMessage> = Vec::new();
    assert_eq!(write_all(&items), "[]");
    assert_eq!(write_all(&items), serde_json::to_string_pretty(&items).unwrap());
  }

  #[test]
  fn writer_matches_pretty_for_one_item() {
    let items = vec![sample_message("a")];
    assert_eq!(write_all(&items), serde_json::to_string_pretty(&items).unwrap());
  }

  #[test]
  fn writer_matches_pretty_for_three_items() {
    let items = vec![sample_message("a"), sample_message("b"), sample_message("c")];
    assert_eq!(write_all(&items), serde_json::to_string_pretty(&items).unwrap());
  }

  #[test]
  fn writer_matches_pretty_for_nested_values() {
    let items = vec![json!({"a": [1, [2, {}], []], "b": {"c": null}}), json!([]), json!("x")];
    assert_eq!(write_all(&items), serde_json::to_string_pretty(&items).unwrap());
  }

  #[test]
  fn from_message_maps_all_fields() {
    let message = Message::builder()
      .message_id("id-1")
      .receipt_handle("handle-1")
      .md5_of_body("body-md5")
      .body("hello")
      .md5_of_message_attributes("attr-md5")
      .attributes(MessageSystemAttributeName::SentTimestamp, "1700000000000")
      .attributes(MessageSystemAttributeName::ApproximateReceiveCount, "2")
      .message_attributes(
        "Text",
        MessageAttributeValue::builder().data_type("String").string_value("value").build().unwrap(),
      )
      .message_attributes(
        "Bytes",
        MessageAttributeValue::builder()
          .data_type("Binary")
          .binary_value(Blob::new(b"hello".to_vec()))
          .build()
          .unwrap(),
      )
      .build();

    let value = serde_json::to_value(PeekedMessage::from(message)).unwrap();
    assert_eq!(
      value,
      json!({
        "MessageId": "id-1",
        "ReceiptHandle": "handle-1",
        "MD5OfBody": "body-md5",
        "Body": "hello",
        "Attributes": {
          "ApproximateReceiveCount": "2",
          "SentTimestamp": "1700000000000",
        },
        "MD5OfMessageAttributes": "attr-md5",
        "MessageAttributes": {
          "Bytes": {"DataType": "Binary", "StringValue": null, "BinaryValue": "aGVsbG8="},
          "Text": {"DataType": "String", "StringValue": "value", "BinaryValue": null},
        },
      })
    );
  }

  #[test]
  fn from_message_without_attributes_yields_empty_maps() {
    let peeked = PeekedMessage::from(Message::builder().build());
    assert_eq!(
      serde_json::to_string(&peeked).unwrap(),
      r#"{"MessageId":null,"ReceiptHandle":null,"MD5OfBody":null,"Body":null,"Attributes":{},"MD5OfMessageAttributes":null,"MessageAttributes":{}}"#
    );
  }
}
