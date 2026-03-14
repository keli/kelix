use chrono::Local;
use crossterm::terminal;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::Instant;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use super::input::byte_index_at_char;

// @chunk tui-client/message-timestamp-prefix
// Add a per-session message timing prefix for incoming scoped messages.
// Format: "[HH:MM:SS +<delta>s]" where delta is time since the previous
// message displayed for the same session in this TUI process.
static SESSION_LAST_SEEN: OnceLock<StdMutex<HashMap<String, Instant>>> = OnceLock::new();

fn now_full_timestamp() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn format_scoped_prefix(session_id: &str) -> String {
    let now_clock = now_full_timestamp();
    let now_mono = Instant::now();

    let map = SESSION_LAST_SEEN.get_or_init(|| StdMutex::new(HashMap::new()));
    let mut guard = map.lock().expect("session timing mutex poisoned");
    let delta = guard
        .insert(session_id.to_string(), now_mono)
        .map(|prev| now_mono.duration_since(prev).as_secs_f64());

    match delta {
        Some(secs) => format!("[{now_clock} +{secs:.1}s]"),
        None => format!("[{now_clock} +--]"),
    }
}
// @end-chunk

pub struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

pub fn char_display_width(c: char) -> usize {
    match c {
        '\u{0000}'..='\u{001F}' | '\u{007F}'..='\u{009F}' => 0,
        '\u{0300}'..='\u{036F}'
        | '\u{1AB0}'..='\u{1AFF}'
        | '\u{1DC0}'..='\u{1DFF}'
        | '\u{20D0}'..='\u{20FF}'
        | '\u{FE20}'..='\u{FE2F}' => 0,
        '\u{1100}'..='\u{115F}'
        | '\u{2329}'..='\u{232A}'
        | '\u{2E80}'..='\u{A4CF}'
        | '\u{AC00}'..='\u{D7A3}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{FE10}'..='\u{FE19}'
        | '\u{FE30}'..='\u{FE6F}'
        | '\u{FF00}'..='\u{FF60}'
        | '\u{FFE0}'..='\u{FFE6}' => 2,
        _ => 1,
    }
}

pub fn str_display_width(s: &str) -> usize {
    s.chars().map(char_display_width).sum()
}

pub async fn redraw_prompt(output_lock: &Arc<Mutex<()>>, input_buf: &str, cursor_char_pos: usize) {
    let total_chars = input_buf.chars().count();
    let cursor_chars = cursor_char_pos.min(total_chars);
    let cursor_byte = byte_index_at_char(input_buf, cursor_chars);
    let trailing_cols = str_display_width(&input_buf[cursor_byte..]);

    let _guard = output_lock.lock().await;
    let mut stdout = tokio::io::stdout();
    let _ = stdout.write_all(b"\x1B[0m").await;
    let _ = stdout.write_all(b"\r\x1B[2K").await;
    let _ = stdout.write_all(b"> ").await;
    let _ = stdout.write_all(input_buf.as_bytes()).await;
    if trailing_cols > 0 {
        let _ = stdout
            .write_all(format!("\x1B[{}D", trailing_cols).as_bytes())
            .await;
    }
    let _ = stdout.flush().await;
}

pub async fn print_line(
    output_lock: &Arc<Mutex<()>>,
    text: &str,
    input_buf: &str,
    cursor_char_pos: usize,
) {
    print_line_raw(
        output_lock,
        &format!("[{}] {text}", now_full_timestamp()),
        input_buf,
        cursor_char_pos,
    )
    .await;
}

async fn print_line_raw(
    output_lock: &Arc<Mutex<()>>,
    text: &str,
    input_buf: &str,
    cursor_char_pos: usize,
) {
    {
        let _guard = output_lock.lock().await;
        let mut stdout = tokio::io::stdout();
        let _ = stdout.write_all(b"\x1B[0m").await;
        let _ = stdout.write_all(b"\r\x1B[2K").await;
        // In raw mode \n only moves down without returning to column 0;
        // replace every \n with \r\n so each line starts at the left edge.
        let normalized = text.replace('\n', "\r\n");
        let _ = stdout.write_all(normalized.as_bytes()).await;
        let _ = stdout.write_all(b"\r\n").await;
        let _ = stdout.flush().await;
    }
    redraw_prompt(output_lock, input_buf, cursor_char_pos).await;
}

pub async fn print_scoped(
    output_lock: &Arc<Mutex<()>>,
    session_id: &str,
    active_session: &str,
    text: &str,
    input_buf: &str,
    cursor_char_pos: usize,
) {
    let prefix = format_scoped_prefix(session_id);
    if session_id == active_session {
        print_line_raw(
            output_lock,
            &format!("{prefix} {text}"),
            input_buf,
            cursor_char_pos,
        )
        .await;
    } else {
        print_line_raw(
            output_lock,
            &format!("{prefix} [session:{session_id}] {text}"),
            input_buf,
            cursor_char_pos,
        )
        .await;
    }
}

pub async fn clear_screen(output_lock: &Arc<Mutex<()>>, input_buf: &str, cursor_char_pos: usize) {
    {
        let _guard = output_lock.lock().await;
        let mut stdout = tokio::io::stdout();
        let _ = stdout.write_all(b"\x1B[2J\x1B[H").await;
        let _ = stdout.flush().await;
    }
    redraw_prompt(output_lock, input_buf, cursor_char_pos).await;
}

pub async fn print_help(output_lock: &Arc<Mutex<()>>, input_buf: &str, cursor_char_pos: usize) {
    print_line(output_lock, "commands:", input_buf, cursor_char_pos).await;
    print_line(
        output_lock,
        "  <text>                          send user_message",
        input_buf,
        cursor_char_pos,
    )
    .await;
    print_line(
        output_lock,
        "  /approve <request_id> <choice>  send approval_response",
        input_buf,
        cursor_char_pos,
    )
    .await;
    print_line(
        output_lock,
        "  /suspend                        request immediate session suspend",
        input_buf,
        cursor_char_pos,
    )
    .await;
    print_line(
        output_lock,
        "  /end                            same as /suspend",
        input_buf,
        cursor_char_pos,
    )
    .await;
    print_line(
        output_lock,
        "  /done                           same as /suspend",
        input_buf,
        cursor_char_pos,
    )
    .await;
    print_line(
        output_lock,
        "  /resume [session_id]            resume suspended session (default: active)",
        input_buf,
        cursor_char_pos,
    )
    .await;
    print_line(
        output_lock,
        "  /session <session_id>            switch active session",
        input_buf,
        cursor_char_pos,
    )
    .await;
    print_line(
        output_lock,
        "  /debug [on|off]                 toggle runtime debug mode",
        input_buf,
        cursor_char_pos,
    )
    .await;
    print_line(
        output_lock,
        "  /help                           show commands",
        input_buf,
        cursor_char_pos,
    )
    .await;
    print_line(
        output_lock,
        "  /quit                           exit local tui client",
        input_buf,
        cursor_char_pos,
    )
    .await;
    print_line(
        output_lock,
        "  /exit                           same as /quit",
        input_buf,
        cursor_char_pos,
    )
    .await;
    print_line(
        output_lock,
        "  Ctrl-C                          exit local tui client",
        input_buf,
        cursor_char_pos,
    )
    .await;
    print_line(
        output_lock,
        "  /clear                          clear terminal screen",
        input_buf,
        cursor_char_pos,
    )
    .await;
    print_line(
        output_lock,
        "  /shutdown                        shut down the gateway process",
        input_buf,
        cursor_char_pos,
    )
    .await;
    print_line(
        output_lock,
        "  //text                          send literal '/text'",
        input_buf,
        cursor_char_pos,
    )
    .await;
}
