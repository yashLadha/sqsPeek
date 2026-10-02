use std::fmt::Display;
use std::io::{self, IsTerminal, Write};
use std::time::{Duration, Instant};

const REDRAW_INTERVAL: Duration = Duration::from_millis(100);

pub struct Progress {
  label: &'static str,
  total: Option<usize>,
  count: usize,
  enabled: bool,
  last_draw: Option<Instant>,
}

impl Progress {
  /// `total` is Some for phases with a known size (e.g. "Purged 120/5000"), None otherwise ("Fetched 120").
  pub fn new(label: &'static str, total: Option<usize>, enabled: bool) -> Self {
    Self { label, total, count: 0, enabled, last_draw: None }
  }

  pub fn add(&mut self, n: usize) {
    self.count += n;
    if !self.enabled {
      return;
    }
    let now = Instant::now();
    if self.last_draw.is_none_or(|last| now.duration_since(last) >= REDRAW_INTERVAL) {
      self.last_draw = Some(now);
      self.draw();
    }
  }

  /// Draws the final count; the line is ended when the progress is dropped.
  pub fn finish(mut self) {
    if self.enabled {
      self.last_draw = Some(Instant::now());
      self.draw();
    }
  }

  /// Prints a line on stderr without it running into the progress line.
  pub fn println(&self, line: impl Display) {
    let mut stderr = io::stderr().lock();
    if self.enabled && self.last_draw.is_some() {
      let _ = write!(stderr, "\r\x1b[2K");
    }
    let _ = writeln!(stderr, "{line}");
    drop(stderr);
    if self.enabled && self.last_draw.is_some() {
      self.draw();
    }
  }

  fn draw(&self) {
    let mut stderr = io::stderr().lock();
    let _ = write!(stderr, "\r{}", self.render());
    let _ = stderr.flush();
  }

  fn render(&self) -> String {
    match self.total {
      Some(total) => format!("{} {}/{total}", self.label, self.count),
      None => format!("{} {}", self.label, self.count),
    }
  }
}

impl Drop for Progress {
  fn drop(&mut self) {
    if self.enabled && self.last_draw.is_some() {
      eprintln!();
    }
  }
}

pub fn enabled(verbose: bool) -> bool {
  !verbose && io::stderr().is_terminal()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn render_without_total() {
    let mut progress = Progress::new("Fetched", None, false);
    progress.add(120);
    assert_eq!(progress.render(), "Fetched 120");
  }

  #[test]
  fn render_with_total() {
    let mut progress = Progress::new("Purged", Some(5000), false);
    progress.add(120);
    assert_eq!(progress.render(), "Purged 120/5000");
  }

  #[test]
  fn count_accumulates_across_adds() {
    let mut progress = Progress::new("Fetched", None, false);
    progress.add(10);
    progress.add(0);
    progress.add(5);
    assert_eq!(progress.count, 15);
    assert_eq!(progress.render(), "Fetched 15");
  }

  #[test]
  fn disabled_progress_counts_without_drawing() {
    let mut progress = Progress::new("Purged", Some(3), false);
    for _ in 0..3 {
      progress.add(1);
    }
    assert_eq!(progress.count, 3);
    assert!(progress.last_draw.is_none());
    progress.finish();
  }
}
