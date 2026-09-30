//! Editor state: the note as a list of lines, mirrored into the UI model.
//!
//! Structural edits (splitting, merging or replacing the active line) always
//! give the UI a fresh row, and with it a fresh text input. That keeps the
//! on-screen keyboard's idea of the text in step with ours.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use scarlet_core::markdown::{self, Kind, LineInfo, TASK_PREFIX};
use scarlet_core::{vault, Result};
use slint::{Model, StyledText, Timer, TimerMode, VecModel};

use crate::{AppWindow, Line, LineKind};

const SAVE_DELAY: Duration = Duration::from_millis(1500);

#[derive(Default)]
pub struct Editor {
    pub model: Rc<VecModel<Line>>,
    raws: RefCell<Vec<String>>,
    infos: RefCell<Vec<LineInfo>>,
    /// Vault root and relative path of the open note.
    note: RefCell<Option<(PathBuf, String)>>,
    trailing_newline: Cell<bool>,
    dirty: Cell<bool>,
    save_timer: Timer,
}

fn blank_info() -> LineInfo {
    LineInfo { kind: Kind::Blank, indent: 0, marker: String::new(), content: String::new() }
}

fn styled(markdown: &str, fallback: &str) -> StyledText {
    StyledText::from_markdown(markdown).unwrap_or_else(|_| StyledText::from_plain_text(fallback))
}

fn row(raw: &str, info: &LineInfo) -> Line {
    let (kind, level) = match info.kind {
        Kind::Blank => (LineKind::Blank, 0),
        Kind::Paragraph => (LineKind::Paragraph, 0),
        Kind::Heading(level) => (LineKind::Heading, level),
        Kind::Bullet => (LineKind::Bullet, 0),
        Kind::Ordered => (LineKind::Ordered, 0),
        Kind::TaskOpen => (LineKind::TaskOpen, 0),
        Kind::TaskDone => (LineKind::TaskDone, 0),
        Kind::Quote => (LineKind::Quote, 0),
        Kind::Fence => (LineKind::Fence, 0),
        Kind::Code => (LineKind::Code, 0),
        Kind::Rule => (LineKind::Rule, 0),
    };
    let content = info.content.as_str();
    let text = match info.kind {
        Kind::Heading(_) | Kind::Fence | Kind::Code | Kind::Rule | Kind::Blank => StyledText::from_plain_text(""),
        Kind::TaskDone => styled(&format!("~~{content}~~"), content),
        _ => styled(content, content),
    };
    Line {
        raw: raw.into(),
        kind,
        level: level.into(),
        indent: info.indent.into(),
        marker: info.marker.as_str().into(),
        plain: content.into(),
        text,
    }
}

fn leading_whitespace(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

impl Editor {
    pub fn open(&self, ui: &AppWindow, root: PathBuf, rel: &str) -> Result<()> {
        let text = vault::read_note(&root, rel)?;
        let mut lines: Vec<String> = text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
        let trailing_newline = lines.len() > 1 && lines.last().is_some_and(String::is_empty);
        if trailing_newline {
            lines.pop();
        }
        self.trailing_newline.set(trailing_newline || text.is_empty());

        let infos = markdown::classify(&lines);
        let rows: Vec<Line> = lines.iter().zip(&infos).map(|(raw, info)| row(raw, info)).collect();
        self.model.set_vec(rows);
        let is_empty = text.trim().is_empty();
        *self.raws.borrow_mut() = lines;
        *self.infos.borrow_mut() = infos;
        *self.note.borrow_mut() = Some((root, rel.to_string()));
        self.dirty.set(false);

        // A brand-new note opens ready for typing.
        ui.set_cursor_offset(0);
        ui.set_active_line(if is_empty { 0 } else { -1 });
        Ok(())
    }

    /// Saves and closes the note.
    pub fn close(&self, ui: &AppWindow) -> Result<()> {
        let result = self.save();
        ui.set_active_line(-1);
        self.model.set_vec(Vec::new());
        self.raws.borrow_mut().clear();
        self.infos.borrow_mut().clear();
        *self.note.borrow_mut() = None;
        result
    }

    pub fn save(&self) -> Result<()> {
        self.save_timer.stop();
        if !self.dirty.get() {
            return Ok(());
        }
        let Some((root, rel)) = self.note.borrow().clone() else { return Ok(()) };
        // Blank lines at the end (tapping below the note adds one) aren't kept.
        let mut text = self.raws.borrow().join("\n").trim_end_matches('\n').to_string();
        if self.trailing_newline.get() {
            text.push('\n');
        }
        vault::write_note(&root, &rel, &text)?;
        self.dirty.set(false);
        Ok(())
    }

    /// Marks the note as changed; `save` runs once typing pauses.
    fn touch(&self, save: impl FnMut() + 'static) {
        self.dirty.set(true);
        self.save_timer.start(TimerMode::SingleShot, SAVE_DELAY, save);
    }

    fn set_raw(&self, index: usize, text: &str) {
        self.raws.borrow_mut()[index] = text.to_string();
    }

    fn insert(&self, index: usize, text: &str) {
        self.raws.borrow_mut().insert(index, text.to_string());
        self.infos.borrow_mut().insert(index, blank_info());
        self.model.insert(index, row(text, &blank_info()));
    }

    fn remove(&self, index: usize) -> String {
        self.infos.borrow_mut().remove(index);
        self.model.remove(index);
        self.raws.borrow_mut().remove(index)
    }

    /// Swaps a line for a fresh row holding `text` and puts the caret in it.
    fn replace(&self, ui: &AppWindow, index: usize, text: &str, cursor: usize) {
        self.remove(index);
        ui.set_cursor_offset(cursor as i32);
        ui.set_active_line(index as i32);
        self.insert(index, text);
    }

    fn activate(&self, ui: &AppWindow, index: usize, cursor: usize) {
        ui.set_cursor_offset(cursor as i32);
        ui.set_active_line(index as i32);
    }

    /// Re-classifies all lines and updates the rows whose look changed.
    fn render(&self) {
        let raws = self.raws.borrow();
        let new_infos = markdown::classify(&raws);
        let mut infos = self.infos.borrow_mut();
        for (index, (raw, info)) in raws.iter().zip(&new_infos).enumerate() {
            let stale = infos[index] != *info || self.model.row_data(index).is_none_or(|r| r.raw != raw.as_str());
            if stale {
                self.model.set_row_data(index, row(raw, info));
            }
        }
        *infos = new_infos;
    }

    fn len(&self) -> usize {
        self.raws.borrow().len()
    }

    fn raw(&self, index: usize) -> String {
        self.raws.borrow()[index].clone()
    }

    pub fn line_activated(&self, ui: &AppWindow, index: usize) {
        if index < self.len() {
            self.activate(ui, index, self.raw(index).len());
        }
    }

    /// The text of the active line changed. A line break in it means the
    /// user pressed Enter (or pasted several lines): split the line there.
    pub fn line_edited(&self, ui: &AppWindow, index: usize, text: &str, save: impl FnMut() + 'static) {
        if index >= self.len() {
            return;
        }
        self.touch(save);
        let parts: Vec<&str> = text.split('\n').collect();
        match parts.as_slice() {
            [line] => self.set_raw(index, line),
            [first, rest] => match markdown::continuation(first) {
                // Enter on an empty list item ends the list instead of continuing it.
                None if rest.is_empty() => self.replace(ui, index, "", 0),
                continuation => {
                    let prefix = continuation.unwrap_or_default();
                    self.set_raw(index, first);
                    self.insert(index + 1, &format!("{prefix}{rest}"));
                    self.activate(ui, index + 1, prefix.len());
                }
            },
            [first, more @ ..] => {
                self.set_raw(index, first);
                for (offset, part) in more.iter().enumerate() {
                    self.insert(index + 1 + offset, part);
                }
                let last = index + more.len();
                self.activate(ui, last, self.raw(last).len());
            }
            [] => {}
        }
        self.render();
    }

    /// Backspace at the start of a line joins it with the line above.
    pub fn line_merged_up(&self, ui: &AppWindow, index: usize, save: impl FnMut() + 'static) {
        if index == 0 || index >= self.len() {
            return;
        }
        self.touch(save);
        let current = self.remove(index);
        let previous = self.raw(index - 1);
        self.replace(ui, index - 1, &format!("{previous}{current}"), previous.len());
        self.render();
    }

    pub fn line_toggled(&self, index: usize, save: impl FnMut() + 'static) {
        if index >= self.len() {
            return;
        }
        if let Some(toggled) = markdown::toggle_task(&self.raw(index)) {
            self.touch(save);
            self.set_raw(index, &toggled);
            self.render();
        }
    }

    /// A tap below the last line: continue writing at the end of the note.
    pub fn tap_end(&self, ui: &AppWindow) {
        let count = self.len();
        if count > 0 && self.raw(count - 1).trim().is_empty() {
            self.activate(ui, count - 1, self.raw(count - 1).len());
        } else {
            self.activate(ui, count, 0);
            self.insert(count, "");
        }
        self.render();
    }

    /// Starts a new task line next to the caret (or at the end of the note)
    /// and leaves the caret right after "- [ ] ".
    pub fn add_task(&self, ui: &AppWindow, save: impl FnMut() + 'static) {
        self.touch(save);
        let count = self.len();
        let active = usize::try_from(ui.get_active_line()).ok().filter(|i| *i < count);
        let anchor = active.or(count.checked_sub(1));
        match anchor {
            Some(index) if self.raw(index).trim().is_empty() => {
                let task = format!("{}{TASK_PREFIX}", leading_whitespace(&self.raw(index)));
                self.replace(ui, index, &task, task.len());
            }
            Some(index) => {
                let task = format!("{}{TASK_PREFIX}", leading_whitespace(&self.raw(index)));
                self.activate(ui, index + 1, task.len());
                self.insert(index + 1, &task);
            }
            None => {
                self.activate(ui, 0, TASK_PREFIX.len());
                self.insert(0, TASK_PREFIX);
            }
        }
        self.render();
    }

    /// Leaves editing mode: every line is rendered again.
    pub fn done(&self, ui: &AppWindow) -> Result<()> {
        ui.set_active_line(-1);
        self.render();
        self.save()
    }
}
