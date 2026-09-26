// output.rs — where command output goes (human text or JSON) and prompts
//
// Every command handler gets an `Output`:
//   outln!(out, "…")   human text on stdout; dropped in --json mode
//   note!(out, "…")    human text on stderr; dropped in --json mode
//   out.json(&value)   the command's JSON document; only in --json mode
//   out.event(&value)  one NDJSON line (streaming commands); only in --json mode
//   out.confirm(…)     the --yes / prompt / refuse logic for destructive commands
//
// So a handler prints its human rendering with outln! and hands its data to
// out.json(); exactly one of the two reaches stdout. Commands that emitted
// nothing in JSON mode get `{"ok":true}` from `finish`, so stdout always
// holds a JSON document. Tests build an `Output` over a buffer (`capture`)
// with an explicit "stdin is a terminal" flag and scripted answers.

use std::fmt;
use std::io::{self, BufRead, IsTerminal, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use serde::Serialize;
use serde_json::json;

use crate::error::CliError;

type Writer = Box<dyn Write + Send>;
type Reader = Box<dyn BufRead + Send>;

pub struct Output {
    json: bool,
    stdin_tty: bool,
    out: Mutex<Writer>,
    err: Mutex<Writer>,
    input: Mutex<Reader>,
    emitted: AtomicBool,
}

/// Human text on stdout (dropped in JSON mode).
macro_rules! outln {
    ($out:expr) => {
        $out.human(format_args!("\n"))
    };
    ($out:expr, $($arg:tt)*) => {
        $out.human(format_args!("{}\n", format_args!($($arg)*)))
    };
}

/// Human text on stdout without a newline (dropped in JSON mode).
macro_rules! out {
    ($out:expr, $($arg:tt)*) => {
        $out.human(format_args!($($arg)*))
    };
}

/// Human text on stderr (dropped in JSON mode).
macro_rules! note {
    ($out:expr, $($arg:tt)*) => {
        $out.note(format_args!("{}\n", format_args!($($arg)*)))
    };
}

pub(crate) use {note, out, outln};

#[cfg(test)]
/// A buffer standing in for stdout or stderr in tests.
#[derive(Clone, Default)]
pub struct Captured(std::sync::Arc<Mutex<Vec<u8>>>);

#[cfg(test)]
impl Captured {
    pub fn text(&self) -> String {
        self.0
            .lock()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    }
}

#[cfg(test)]
impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("poisoned"))?
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Output {
    /// The real stdout/stderr/stdin.
    pub fn stdio(json: bool) -> Self {
        Self::new(
            json,
            io::stdin().is_terminal(),
            Box::new(io::stdout()),
            Box::new(io::stderr()),
            Box::new(io::BufReader::new(io::stdin())),
        )
    }

    pub fn new(json: bool, stdin_tty: bool, out: Writer, err: Writer, input: Reader) -> Self {
        Self {
            json,
            stdin_tty,
            out: Mutex::new(out),
            err: Mutex::new(err),
            input: Mutex::new(input),
            emitted: AtomicBool::new(false),
        }
    }

    /// An `Output` writing into buffers; `answers` is what a user would type.
    #[cfg(test)]
    pub fn capture(json: bool, stdin_tty: bool, answers: &str) -> (Self, Captured, Captured) {
        let stdout = Captured::default();
        let stderr = Captured::default();
        let out = Self::new(
            json,
            stdin_tty,
            Box::new(stdout.clone()),
            Box::new(stderr.clone()),
            Box::new(io::Cursor::new(answers.as_bytes().to_vec())),
        );
        (out, stdout, stderr)
    }

    pub fn is_json(&self) -> bool {
        self.json
    }

    fn write_out(&self, text: &str) {
        if let Ok(mut w) = self.out.lock() {
            let _ = w.write_all(text.as_bytes());
            let _ = w.flush();
        }
    }

    fn write_err(&self, text: &str) {
        if let Ok(mut w) = self.err.lock() {
            let _ = w.write_all(text.as_bytes());
            let _ = w.flush();
        }
    }

    /// Human text on stdout; nothing in JSON mode. Use `outln!`.
    pub fn human(&self, args: fmt::Arguments<'_>) {
        if !self.json {
            self.write_out(&args.to_string());
        }
    }

    /// Human text on stderr; nothing in JSON mode. Use `note!`.
    pub fn note(&self, args: fmt::Arguments<'_>) {
        if !self.json {
            self.write_err(&args.to_string());
        }
    }

    /// Text on stdout in either mode (completion scripts).
    pub fn raw(&self, text: &str) {
        self.emitted.store(true, Ordering::Relaxed);
        self.write_out(text);
    }

    /// The command's JSON document (JSON mode only).
    pub fn json<T: Serialize + ?Sized>(&self, value: &T) -> Result<()> {
        if self.json {
            let line = serde_json::to_string(value)?;
            self.emitted.store(true, Ordering::Relaxed);
            self.write_out(&format!("{line}\n"));
        }
        Ok(())
    }

    /// One NDJSON event line (JSON mode only).
    pub fn event<T: Serialize + ?Sized>(&self, value: &T) -> Result<()> {
        self.json(value)
    }

    /// Standard result of a mutation without a natural resource to print:
    /// `{"ok":true,"action":"delete","resource":"app","id":"web","data":…}`.
    pub fn done(
        &self,
        action: &str,
        resource: &str,
        id: &str,
        data: serde_json::Value,
    ) -> Result<()> {
        self.json(&json!({
            "ok": true,
            "action": action,
            "resource": resource,
            "id": id,
            "data": data,
        }))
    }

    /// Called after a successful command: guarantees a JSON document.
    pub fn finish(&self) {
        if self.json && !self.emitted.load(Ordering::Relaxed) {
            self.write_out("{\"ok\":true}\n");
        }
    }

    /// Print `prompt` on stderr and read one line of input.
    pub fn prompt_line(&self, prompt: &str) -> Result<String> {
        self.write_err(prompt);
        let mut line = String::new();
        self.input
            .lock()
            .map_err(|_| anyhow::anyhow!("stdin lock poisoned"))?
            .read_line(&mut line)?;
        Ok(line.trim_end_matches(['\n', '\r']).to_string())
    }

    /// The refusal half of [`Output::confirm`], for commands that look
    /// something up before asking: call it first so a missing --yes without
    /// a terminal is refused (exit 2) before any request is made.
    pub fn can_confirm(&self, yes: bool, action: &str) -> Result<()> {
        if yes || self.stdin_tty {
            return Ok(());
        }
        Err(CliError::usage(format!(
            "Refusing to {action} without --yes (stdin is not a terminal)."
        ))
        .with_hint("Pass --yes (-y) to confirm.")
        .into())
    }

    /// Gate a destructive action. With `yes`, go ahead. Otherwise ask
    /// `question` on stderr when stdin is a terminal (anything but y/yes
    /// cancels), and refuse with a usage error (exit 2) when it is not:
    /// "Refusing to <action> without --yes (stdin is not a terminal)."
    pub fn confirm(&self, yes: bool, question: &str, action: &str) -> Result<()> {
        if yes {
            return Ok(());
        }
        self.can_confirm(false, action)?;
        let answer = self.prompt_line(&format!("{question} [y/N] "))?;
        match answer.trim().to_lowercase().as_str() {
            "y" | "yes" => Ok(()),
            _ => Err(CliError::aborted().into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error;

    #[test]
    fn human_text_and_json_never_mix() {
        let (out, stdout, stderr) = Output::capture(true, false, "");
        outln!(out, "table row");
        note!(out, "progress");
        out.json(&json!({"a": 1})).unwrap_or_default();
        assert_eq!(stdout.text(), "{\"a\":1}\n");
        assert_eq!(stderr.text(), "");

        let (out, stdout, stderr) = Output::capture(false, false, "");
        outln!(out, "table row");
        note!(out, "progress");
        out.json(&json!({"a": 1})).unwrap_or_default();
        out.finish();
        assert_eq!(stdout.text(), "table row\n");
        assert_eq!(stderr.text(), "progress\n");
    }

    #[test]
    fn finish_guarantees_a_json_document() {
        let (out, stdout, _) = Output::capture(true, false, "");
        out.finish();
        assert_eq!(stdout.text(), "{\"ok\":true}\n");
    }

    #[test]
    fn confirm_refuses_without_a_terminal() {
        let (out, _, _) = Output::capture(false, false, "y\n");
        let err = out
            .confirm(false, "Delete app web?", "delete app web")
            .err()
            .unwrap_or_else(|| anyhow::anyhow!("expected a refusal"));
        assert_eq!(error::exit_code(&err), 2);
        assert_eq!(
            err.to_string(),
            "Refusing to delete app web without --yes (stdin is not a terminal)."
        );
        assert!(
            out.confirm(true, "Delete app web?", "delete app web")
                .is_ok()
        );
    }

    #[test]
    fn confirm_prompts_on_a_terminal() {
        let (out, stdout, stderr) = Output::capture(false, true, "y\n");
        assert!(
            out.confirm(false, "Delete app web?", "delete app web")
                .is_ok()
        );
        assert_eq!(stderr.text(), "Delete app web? [y/N] ");
        assert_eq!(stdout.text(), "");

        let (out, _, _) = Output::capture(false, true, "n\n");
        let err = out
            .confirm(false, "Delete app web?", "delete app web")
            .err()
            .unwrap_or_else(|| anyhow::anyhow!("expected a cancel"));
        assert_eq!(error::exit_code(&err), 1);
    }
}
