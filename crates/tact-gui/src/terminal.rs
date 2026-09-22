//! An embedded terminal, backed by a real PTY.
//!
//! The work pane's `Terminal` tab runs the user's own shell in a pseudo
//! terminal and renders the screen `vt100` parses out of its output. That
//! distinction matters: this is a terminal, not a command runner. A command
//! runner would re-implement what the shell already does (job control, pipes,
//! prompts, `cd`) and would be wrong about all of it. Here the child is the
//! shell, the escape sequences are interpreted by a parser, and the pane only
//! draws the grid and forwards keystrokes.
//!
//! Nothing here runs until the user asks for it. The pane renders an explicit
//! start control because spawning a shell is a side effect, and a pane that
//! spawns one merely by being looked at would also spawn one in every test that
//! walks the work-pane tabs.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Columns and rows a fresh pane asks the PTY for.
///
/// The pane re-measures on its first frame, so this only has to be a sane
/// starting size for the shell's initial prompt.
const DEFAULT_COLS: u16 = 90;
const DEFAULT_ROWS: u16 = 24;

/// Lines `vt100` keeps above the visible grid.
const SCROLLBACK: usize = 4_000;

/// Most output one `poll` will apply before returning.
///
/// A chatty child (`yes`, a recursive `find`, a large build) can keep the
/// channel non-empty forever, and `poll` runs on the render path; without a
/// ceiling the drain loop would hold the frame until the producer stopped.
/// The remainder stays queued for the next poll.
const POLL_BUDGET_BYTES: usize = 256 * 1024;

/// A colour as the terminal reported it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TermColor {
    /// The terminal's own default, which the pane maps to the theme.
    Default,
    /// The 256-colour palette index.
    Indexed(u8),
    /// A direct colour.
    Rgb(u8, u8, u8),
}

/// One run of adjacent cells that share a style.
///
/// Runs rather than cells because a `Div` per character would put a few
/// thousand elements on screen for one full grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TermRun {
    pub text: String,
    pub fg: TermColor,
    pub bg: TermColor,
    pub bold: bool,
    pub underline: bool,
    pub inverse: bool,
}

/// The VT screen, the PTY, and the child that owns it.
pub(crate) struct TerminalPane {
    /// Kept alive for `resize`; dropping it closes the PTY.
    master: Box<dyn MasterPty + Send>,
    /// The child's stdin.
    writer: Box<dyn Write + Send>,
    /// Output from the reader thread.
    output: Receiver<Vec<u8>>,
    parser: vt100::Parser,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// The command line shown in the pane header.
    program: String,
    /// Set once the child has been reaped, so the pane can say so instead of
    /// looking like a dead terminal.
    exited: Option<u32>,
}

impl TerminalPane {
    /// Start the user's shell in `workdir`.
    ///
    /// `$SHELL` is what the user actually uses; `/bin/sh` is the floor when the
    /// environment does not say.
    pub(crate) fn spawn(workdir: Option<&Path>) -> anyhow::Result<Self> {
        // `$SHELL` is what the user actually uses, but it is not guaranteed to
        // be set or to point at something that still exists. An empty or stale
        // value must not turn Start into a failure while `/bin/sh` is sitting
        // right there.
        let program = std::env::var("SHELL")
            .ok()
            .filter(|path| !path.is_empty() && Path::new(path).exists())
            .unwrap_or_else(|| "/bin/sh".to_string());
        Self::spawn_program(&program, &[], workdir)
    }

    /// Start an explicit program. Tests use this to run something bounded.
    pub(crate) fn spawn_program(
        program: &str,
        args: &[&str],
        workdir: Option<&Path>,
    ) -> anyhow::Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: DEFAULT_ROWS,
            cols: DEFAULT_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut command = CommandBuilder::new(program);
        command.args(args);
        if let Some(dir) = workdir {
            command.cwd(dir);
        }
        // The child inherits the user's environment; a terminal that starts
        // with a different `PATH` than their login shell would be a trap.
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");

        let child = pair.slave.spawn_command(command)?;
        // The slave is only needed to hand to the child; holding it open would
        // keep the PTY from reporting EOF when the child exits.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        // A blocking read on a dedicated thread, because the PTY has no async
        // handle and parking a GPUI task on it would stall the executor.
        let (sender, output) = mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("tact-gui-pty".to_string())
            .spawn(move || {
                let mut buffer = [0_u8; 8192];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => {
                            if sender.send(buffer[..read].to_vec()).is_err() {
                                break;
                            }
                        }
                    }
                }
            })?;

        let program_label = if args.is_empty() {
            program.to_string()
        } else {
            format!("{program} {}", args.join(" "))
        };

        Ok(Self {
            master: pair.master,
            writer,
            output,
            parser: vt100::Parser::new(DEFAULT_ROWS, DEFAULT_COLS, SCROLLBACK),
            child,
            program: program_label,
            exited: None,
        })
    }

    /// Feed everything the reader thread has produced into the VT parser.
    ///
    /// Returns the number of bytes applied, so the caller can skip a redraw
    /// when the terminal has been idle.
    pub(crate) fn poll(&mut self) -> usize {
        let mut applied = 0;
        while applied < POLL_BUDGET_BYTES {
            let Ok(bytes) = self.output.try_recv() else {
                break;
            };
            applied += bytes.len();
            self.parser.process(&bytes);
        }
        if self.exited.is_none()
            && let Ok(Some(status)) = self.child.try_wait()
        {
            self.exited = Some(status.exit_code());
        }
        applied
    }

    /// Forward raw input to the child's stdin.
    pub(crate) fn write(&mut self, bytes: &[u8]) {
        if self.exited.is_some() {
            return;
        }
        // A write to a PTY whose child just died is an ordinary race, not an
        // error worth surfacing: the pane already reports the exit.
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// Re-measure the PTY and the VT grid together.
    ///
    /// Both have to move: resizing only the parser would wrap the shell's
    /// output at the wrong column, and resizing only the PTY would draw a grid
    /// that disagrees with what the shell thinks the window is.
    pub(crate) fn resize(&mut self, cols: u16, rows: u16) {
        let cols = cols.max(20);
        let rows = rows.max(5);
        let (screen_rows, screen_cols) = self.parser.screen().size();
        if cols == screen_cols && rows == screen_rows {
            return;
        }
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        self.parser.screen_mut().set_size(rows, cols);
    }

    pub(crate) fn cols(&self) -> u16 {
        self.parser.screen().size().1
    }

    pub(crate) fn rows(&self) -> u16 {
        self.parser.screen().size().0
    }

    pub(crate) fn program(&self) -> &str {
        &self.program
    }

    pub(crate) fn exited(&self) -> Option<u32> {
        self.exited
    }

    /// The visible grid as text, for the pane's accessible name and for tests.
    pub(crate) fn contents(&self) -> String {
        self.parser.screen().contents()
    }

    /// The cursor position, so the pane can draw the block the shell is asking
    /// for.
    pub(crate) fn cursor(&self) -> (u16, u16) {
        self.parser.screen().cursor_position()
    }

    /// The styled runs making up one screen row.
    pub(crate) fn row_runs(&self, row: u16) -> Vec<TermRun> {
        runs_from_screen(self.parser.screen(), row)
    }

    /// The styled runs covering `start..end` of one screen row.
    ///
    /// The cursor splits a row into two of these, so a block cursor is drawn in
    /// the cell the shell actually put it in rather than appended to the line.
    pub(crate) fn row_runs_between(&self, row: u16, start: u16, end: u16) -> Vec<TermRun> {
        runs_from_screen_range(self.parser.screen(), row, start, end)
    }

    /// The character under the cursor, for the block.
    pub(crate) fn cell_text(&self, row: u16, col: u16) -> String {
        self.parser
            .screen()
            .cell(row, col)
            .map(|cell| cell.contents().to_string())
            .unwrap_or_default()
    }
}

impl Drop for TerminalPane {
    fn drop(&mut self) {
        // The shell is this pane's child; closing the window must not leave it
        // running, and neither must anything the shell started in the
        // background. `portable-pty` spawns the child as a session leader, so
        // its pid is also its process-group id: signalling the group reaches
        // jobs the shell left running, which a signal to the shell alone would
        // miss. Both calls are harmless once the child has already exited.
        #[cfg(unix)]
        if let Some(pid) = self.child.process_id() {
            // SAFETY: `killpg` is a plain libc call; the pid came from the child
            // this pane spawned, and a group that no longer exists returns an
            // error we ignore.
            unsafe {
                libc::killpg(pid as libc::pid_t, libc::SIGHUP);
            }
        }
        let _ = self.child.kill();
    }
}

/// Group one screen row into same-style runs.
///
/// Trailing blank default-styled cells are dropped so a mostly-empty row does
/// not paint the full pane width with spaces — the pane paints its own
/// background underneath.
pub(crate) fn runs_from_screen(screen: &vt100::Screen, row: u16) -> Vec<TermRun> {
    let (_, cols) = screen.size();
    runs_from_screen_range(screen, row, 0, cols)
}

/// Group a cell range of one screen row into same-style runs.
pub(crate) fn runs_from_screen_range(
    screen: &vt100::Screen,
    row: u16,
    start: u16,
    end: u16,
) -> Vec<TermRun> {
    let (_, cols) = screen.size();
    let mut runs: Vec<TermRun> = Vec::new();
    for col in start.min(cols)..end.min(cols) {
        let Some(cell) = screen.cell(row, col) else {
            continue;
        };
        let run = TermRun {
            text: cell.contents().to_string(),
            fg: convert(cell.fgcolor()),
            bg: convert(cell.bgcolor()),
            bold: cell.bold(),
            underline: cell.underline(),
            inverse: cell.inverse(),
        };
        match runs.last_mut() {
            Some(last) if same_style(last, &run) => last.text.push_str(&run.text),
            _ => runs.push(run),
        }
    }
    while runs
        .last()
        .is_some_and(|run| run.text.trim().is_empty() && run.bg == TermColor::Default)
    {
        runs.pop();
    }
    runs
}

fn same_style(a: &TermRun, b: &TermRun) -> bool {
    a.fg == b.fg
        && a.bg == b.bg
        && a.bold == b.bold
        && a.underline == b.underline
        && a.inverse == b.inverse
}

fn convert(color: vt100::Color) -> TermColor {
    match color {
        vt100::Color::Default => TermColor::Default,
        vt100::Color::Idx(index) => TermColor::Indexed(index),
        vt100::Color::Rgb(r, g, b) => TermColor::Rgb(r, g, b),
    }
}

/// Map a keystroke onto the bytes a terminal would send.
///
/// Terminal input is not text input: `Enter` is a carriage return, `Ctrl-C` is
/// a single control byte, and the arrows are escape sequences. Returning
/// `None` means the key has no terminal encoding and should not be forwarded.
pub(crate) fn key_bytes(keystroke: &gpui_kit::Keystroke) -> Option<Vec<u8>> {
    let key = keystroke.key.as_str();
    let ctrl = keystroke.modifiers.control;

    // Control combinations first: `Ctrl-C` has to reach the child as one byte,
    // not as the letter `c`, or the shell never sees the interrupt.
    if ctrl && let Some(byte) = control_byte(key) {
        return Some(vec![byte]);
    }
    // Modified keys with no control encoding fall through to the plain key
    // table below, which is what terminals do.

    let sequence: &[u8] = match key {
        "enter" | "return" => b"\r",
        "tab" => b"\t",
        "backspace" => b"\x7f",
        "escape" => b"\x1b",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "pageup" => b"\x1b[5~",
        "pagedown" => b"\x1b[6~",
        "delete" => b"\x1b[3~",
        "insert" => b"\x1b[2~",
        "space" => b" ",
        _ => {
            // Anything else goes through the typed character, so a non-ASCII
            // layout still reaches the child as the character the user meant.
            let text = keystroke.key_char.as_deref()?;
            return Some(text.as_bytes().to_vec());
        }
    };
    Some(sequence.to_vec())
}

/// `Ctrl-A` … `Ctrl-Z` (plus the handful of punctuation combinations terminals
/// define) as the control byte the child expects.
fn control_byte(key: &str) -> Option<u8> {
    if let [letter] = key.as_bytes()
        && letter.is_ascii_alphabetic()
    {
        return Some(letter.to_ascii_uppercase() - b'A' + 1);
    }
    let byte = match key {
        "@" | "2" => 0,
        "[" | "3" => 27,
        "\\" | "4" => 28,
        "]" | "5" => 29,
        "^" | "6" => 30,
        "_" | "7" | "/" => 31,
        "?" | "8" => 127,
        _ => return None,
    };
    Some(byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen_with(bytes: &[u8]) -> vt100::Parser {
        let mut parser = vt100::Parser::new(4, 20, 0);
        parser.process(bytes);
        parser
    }

    /// The whole path — PTY, reader thread, VT parser, grid — on one bounded
    /// child. `/bin/sh` is not guaranteed on every host, so a missing shell
    /// skips rather than fails.
    #[test]
    fn a_pty_childs_output_reaches_the_grid() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let Ok(mut pane) = TerminalPane::spawn_program("/bin/sh", &["-c", "printf hello"], None)
        else {
            return;
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut contents = String::new();
        while std::time::Instant::now() < deadline {
            pane.poll();
            contents = pane.parser.screen().contents();
            if contents.contains("hello") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            contents.contains("hello"),
            "the child's output reached the grid: {contents:?}"
        );
    }

    #[test]
    fn a_plain_row_becomes_one_run() {
        let parser = screen_with(b"hello");
        let runs = runs_from_screen(parser.screen(), 0);
        assert_eq!(runs.len(), 1, "one style, one run: {runs:?}");
        assert_eq!(runs[0].text.trim_end(), "hello");
        assert_eq!(runs[0].fg, TermColor::Default);
    }

    #[test]
    fn a_colour_change_splits_the_run() {
        // Red "AB", then default "CD".
        let parser = screen_with(b"\x1b[31mAB\x1b[0mCD");
        let runs = runs_from_screen(parser.screen(), 0);
        assert_eq!(runs.len(), 2, "the style break splits the row: {runs:?}");
        assert_eq!(runs[0].text, "AB");
        assert_eq!(runs[0].fg, TermColor::Indexed(1));
        assert_eq!(runs[1].text.trim_end(), "CD");
        assert_eq!(runs[1].fg, TermColor::Default);
    }

    #[test]
    fn a_direct_colour_survives_the_round_trip() {
        let parser = screen_with(b"\x1b[38;2;10;20;30mX");
        let runs = runs_from_screen(parser.screen(), 0);
        assert_eq!(runs[0].fg, TermColor::Rgb(10, 20, 30));
    }

    #[test]
    fn bold_and_underline_are_kept_separate_from_colour() {
        let parser = screen_with(b"\x1b[1mB\x1b[0m\x1b[4mU");
        let runs = runs_from_screen(parser.screen(), 0);
        assert!(runs[0].bold, "bold is its own attribute: {runs:?}");
        assert!(runs.last().unwrap().underline);
    }

    #[test]
    fn trailing_blank_cells_are_dropped() {
        let parser = screen_with(b"ab");
        let runs = runs_from_screen(parser.screen(), 0);
        assert_eq!(runs.len(), 1);
        assert!(
            runs[0].text.len() <= 3,
            "the row does not carry the empty tail: {:?}",
            runs[0].text
        );
    }

    #[test]
    fn cursor_movement_moves_the_next_run() {
        // Write "ab", return, and write "c" at column 0 of the next row.
        let parser = screen_with(b"ab\r\nc");
        let row0 = runs_from_screen(parser.screen(), 0);
        let row1 = runs_from_screen(parser.screen(), 1);
        assert_eq!(row0[0].text.trim_end(), "ab");
        assert_eq!(row1[0].text.trim_end(), "c");
    }

    fn keystroke(key: &str, key_char: Option<&str>, ctrl: bool) -> gpui_kit::Keystroke {
        gpui_kit::Keystroke {
            key: key.to_string(),
            key_char: key_char.map(str::to_string),
            modifiers: gpui_kit::Modifiers {
                control: ctrl,
                ..Default::default()
            },
        }
    }

    #[test]
    fn printable_keys_send_their_character() {
        assert_eq!(
            key_bytes(&keystroke("a", Some("a"), false)).unwrap(),
            b"a".to_vec()
        );
        assert_eq!(
            key_bytes(&keystroke("space", Some(" "), false)).unwrap(),
            b" ".to_vec()
        );
    }

    #[test]
    fn enter_and_backspace_use_their_terminal_bytes() {
        assert_eq!(
            key_bytes(&keystroke("enter", None, false)).unwrap(),
            b"\r".to_vec()
        );
        assert_eq!(
            key_bytes(&keystroke("backspace", None, false)).unwrap(),
            b"\x7f".to_vec()
        );
    }

    #[test]
    fn arrows_are_escape_sequences() {
        assert_eq!(
            key_bytes(&keystroke("up", None, false)).unwrap(),
            b"\x1b[A".to_vec()
        );
        assert_eq!(
            key_bytes(&keystroke("pagedown", None, false)).unwrap(),
            b"\x1b[6~".to_vec()
        );
    }

    #[test]
    fn control_keys_become_one_byte() {
        assert_eq!(
            key_bytes(&keystroke("c", Some("c"), true)).unwrap(),
            b"\x03".to_vec(),
            "Ctrl-C has to be the interrupt byte, not a letter"
        );
        assert_eq!(
            key_bytes(&keystroke("d", Some("d"), true)).unwrap(),
            b"\x04".to_vec()
        );
        assert_eq!(
            key_bytes(&keystroke("[", Some("["), true)).unwrap(),
            b"\x1b".to_vec()
        );
    }

    #[test]
    fn an_unencodable_key_is_not_forwarded() {
        assert!(key_bytes(&keystroke("f5", None, false)).is_none());
    }
}
