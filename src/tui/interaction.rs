use crate::interaction::{
    AskAnswer, AskCancelReason, AskQuestion, AskRequest, AskResult, ConfirmationRequest,
    HumanInteraction,
};
use async_trait::async_trait;
use crossterm::{
    cursor::{Hide, MoveToColumn, MoveUp, Show},
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{self, Clear, ClearType},
};
use std::io::{self, BufRead, IsTerminal, Write};
use std::sync::Arc;
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Default)]
pub struct TerminalInteraction {
    gate: Arc<tokio::sync::Mutex<()>>,
}

impl TerminalInteraction {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl HumanInteraction for TerminalInteraction {
    async fn ask(&self, request: &AskRequest) -> AskResult {
        if request.validate().is_err() {
            return cancelled(AskCancelReason::Unavailable);
        }
        let guard = self.gate.clone().lock_owned().await;
        let request = request.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let result = if use_card() {
                ask_card(&request)
            } else {
                ask_plain(&request, &mut io::stdin().lock(), &mut io::stderr().lock())
            };
            result.unwrap_or_else(|_| cancelled(AskCancelReason::Unavailable))
        })
        .await
        .unwrap_or_else(|_| cancelled(AskCancelReason::Unavailable))
    }

    async fn confirm(&self, request: &ConfirmationRequest) -> bool {
        let guard = self.gate.clone().lock_owned().await;
        let request = request.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            if use_card() {
                confirm_card(&request)
            } else {
                confirm_plain(&request, &mut io::stdin().lock(), &mut io::stderr().lock())
            }
            .unwrap_or(false)
        })
        .await
        .unwrap_or(false)
    }
}

fn use_card() -> bool {
    io::stdin().is_terminal()
        && io::stderr().is_terminal()
        && std::env::var("TERM").map_or(true, |t| t != "dumb")
}

fn cancelled(reason: AskCancelReason) -> AskResult {
    AskResult::Cancelled { reason }
}

pub(crate) fn safe_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\n' => out.push(c),
            c if c.is_control()
                || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') =>
            {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn read_line(input: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    while line.ends_with(['\r', '\n']) {
        line.pop();
    }
    Ok(Some(line))
}

fn ask_plain(
    r: &AskRequest,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> io::Result<AskResult> {
    let mut answers = Vec::new();
    for (i, q) in r.questions.iter().enumerate() {
        writeln!(
            out,
            "需要补充 · {} / {}{}",
            i + 1,
            r.questions.len(),
            q.header
                .as_ref()
                .map(|h| format!(" · {}", safe_text(h)))
                .unwrap_or_default()
        )?;
        writeln!(out, "{}", safe_text(&q.question))?;
        for (n, o) in q.options.iter().enumerate() {
            writeln!(
                out,
                "{}. {}{}",
                n + 1,
                safe_text(&o.label),
                if q.recommended.as_ref() == Some(&o.id) {
                    " [推荐]"
                } else {
                    ""
                }
            )?;
            if let Some(d) = &o.description {
                writeln!(out, "   {}", safe_text(d))?;
            }
        }
        if !q.options.is_empty() {
            writeln!(out, "0. 自定义回答（输入 0 后另读一行）")?;
        }
        loop {
            write!(
                out,
                "{}: ",
                if q.options.is_empty() {
                    "回答（/cancel 取消）"
                } else if q.multi {
                    "编号，以逗号分隔（/cancel 取消）"
                } else {
                    "编号（/cancel 取消）"
                }
            )?;
            out.flush()?;
            let Some(line) = read_line(input)? else {
                return Ok(cancelled(AskCancelReason::Eof));
            };
            if line.trim() == "/cancel" {
                return Ok(cancelled(AskCancelReason::Cancelled));
            }
            let mut answer = AskAnswer {
                question_id: q.id.clone(),
                selected: Vec::new(),
                custom: None,
            };
            if q.options.is_empty() {
                answer.custom = Some(line);
            } else {
                let numbers = line
                    .split(',')
                    .map(|s| s.trim().parse::<usize>())
                    .collect::<Result<Vec<_>, _>>();
                let Ok(numbers) = numbers else {
                    writeln!(out, "无效编号，请重试")?;
                    continue;
                };
                if numbers.is_empty()
                    || (!q.multi && numbers.len() != 1)
                    || numbers.iter().any(|n| *n > q.options.len())
                {
                    writeln!(out, "无效编号，请重试")?;
                    continue;
                }
                let mut custom = false;
                let mut duplicate = false;
                for n in numbers {
                    if n == 0 {
                        duplicate |= custom;
                        custom = true;
                    } else {
                        let id = &q.options[n - 1].id;
                        duplicate |= answer.selected.contains(id);
                        answer.selected.push(id.clone());
                    }
                }
                if duplicate {
                    writeln!(out, "编号不能重复")?;
                    continue;
                }
                if custom {
                    write!(out, "自定义回答: ")?;
                    out.flush()?;
                    let Some(line) = read_line(input)? else {
                        return Ok(cancelled(AskCancelReason::Eof));
                    };
                    if line.trim() == "/cancel" {
                        return Ok(cancelled(AskCancelReason::Cancelled));
                    }
                    answer.custom = Some(line);
                }
            }
            if validate_one(q, &answer).is_err() {
                writeln!(out, "回答不能为空，最多 2048 字符")?;
                continue;
            }
            answers.push(answer);
            break;
        }
    }
    for line in summary(r, &answers) {
        writeln!(out, "{}", safe_text(&line))?;
    }
    Ok(AskResult::Answered { answers })
}

fn validate_one(q: &AskQuestion, a: &AskAnswer) -> Result<(), String> {
    AskRequest {
        questions: vec![q.clone()],
    }
    .validate_answers(std::slice::from_ref(a))
}

fn confirmation_details(r: &ConfirmationRequest) -> String {
    let mut s = format!("工具: {}\n动作: {}\n{}", r.tool_name, r.summary, r.details);
    if let Some(risk) = &r.risk_label {
        s.push_str(&format!("\n风险: {risk}"));
    }
    if let Some(reason) = &r.reason {
        s.push_str(&format!("\n原因: {reason}"));
    }
    for missing in &r.missing_evidence {
        s.push_str(&format!("\n缺失事实: {missing}"));
    }
    if let Some(p) = &r.preview {
        s.push_str(&format!("\n文件变化:\n{p}"));
    }
    s
}

fn confirm_plain(
    r: &ConfirmationRequest,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> io::Result<bool> {
    writeln!(out, "需要确认\n{}", safe_text(&confirmation_details(r)))?;
    write!(out, "仅允许本次？ [y/N] ")?;
    out.flush()?;
    Ok(read_line(input)?.is_some_and(|line| line.trim().eq_ignore_ascii_case("y")))
}

static TERMINAL_ACTIVE: parking_lot::Mutex<bool> = parking_lot::Mutex::new(false);

pub(crate) fn restore_terminal_interaction() {
    let mut active = TERMINAL_ACTIVE.lock();
    if *active {
        let _ = execute!(io::stderr(), DisableBracketedPaste, Show);
        let _ = terminal::disable_raw_mode();
        *active = false;
    }
}

struct TerminalGuard;
impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        let mut active = TERMINAL_ACTIVE.lock();
        terminal::enable_raw_mode()?;
        *active = true;
        if let Err(e) = execute!(io::stderr(), EnableBracketedPaste, Hide) {
            drop(active);
            restore_terminal_interaction();
            return Err(e);
        }
        Ok(Self)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal_interaction();
    }
}

fn terminal_size() -> (u16, u16) {
    #[cfg(unix)]
    {
        let mut size = unsafe { std::mem::zeroed::<libc::winsize>() };
        if unsafe { libc::ioctl(libc::STDERR_FILENO, libc::TIOCGWINSZ, &mut size) } == 0
            && size.ws_col > 0
            && size.ws_row > 0
        {
            return (size.ws_col, size.ws_row);
        }
    }
    terminal::size().unwrap_or((80, 24))
}

fn read_event() -> io::Result<Event> {
    let size = terminal_size();
    loop {
        if event::poll(std::time::Duration::from_millis(50))? {
            return event::read();
        }
        let current = terminal_size();
        if current != size {
            return Ok(Event::Resize(current.0, current.1));
        }
    }
}

fn wrap(value: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut result = Vec::new();
    for line in value.split('\n') {
        let mut current = String::new();
        let mut used = 0;
        for ch in line.chars() {
            let w = ch.width().unwrap_or(0);
            if used + w > width && !current.is_empty() {
                result.push(std::mem::take(&mut current));
                used = 0;
            }
            current.push(ch);
            used += w;
        }
        result.push(current);
    }
    result
}

#[derive(Default)]
struct Card {
    occupied: usize,
    scroll: usize,
}

impl Card {
    fn clear(&mut self) -> io::Result<()> {
        let mut out = io::stderr().lock();
        if self.occupied > 0 {
            execute!(out, MoveUp(self.occupied as u16), MoveToColumn(0))?;
            for i in 0..self.occupied {
                execute!(out, Clear(ClearType::CurrentLine))?;
                if i + 1 < self.occupied {
                    write!(out, "\r\n")?;
                }
            }
            if self.occupied > 1 {
                execute!(out, MoveUp((self.occupied - 1) as u16))?;
            }
            execute!(out, MoveToColumn(0))?;
        }
        self.occupied = 0;
        out.flush()
    }

    fn render(
        &mut self,
        title: &str,
        content: &[String],
        footer: &str,
        focus: Option<usize>,
    ) -> io::Result<()> {
        self.clear()?;
        let (cols, rows) = terminal_size();
        let narrow = cols < 40;
        let width = usize::from(cols).saturating_sub(4).clamp(1, 88);
        let inner = width.saturating_sub(if narrow { 0 } else { 2 }).max(1);
        let mut lines = Vec::new();
        let mut focus_line = None;
        for (i, line) in content.iter().enumerate() {
            if focus == Some(i) {
                focus_line = Some(lines.len());
            }
            lines.extend(wrap(&safe_text(line), inner));
        }
        let height = usize::from(rows).saturating_sub(2).max(1);
        let body_height = height.saturating_sub(2);
        if let Some(f) = focus_line {
            if f < self.scroll {
                self.scroll = f;
            }
            if f >= self.scroll + body_height && body_height > 0 {
                self.scroll = f + 1 - body_height;
            }
        }
        self.scroll = self.scroll.min(lines.len().saturating_sub(body_height));
        let mut rendered = Vec::new();
        if height > 1 {
            rendered.push(format!(
                "\x1b[36m{}{}\x1b[0m",
                if narrow { "" } else { "╭─ " },
                wrap(&safe_text(title), width.saturating_sub(3).max(1))
                    .first()
                    .cloned()
                    .unwrap_or_default()
            ));
        }
        for line in lines.iter().skip(self.scroll).take(body_height) {
            rendered.push(format!("{}{}", if narrow { "" } else { "│ " }, line));
        }
        let compact = if footer.contains("y 允许") {
            "Enter y/n d Esc"
        } else if footer.contains("浏览") {
            "PgUp/PgDn Esc"
        } else if footer.contains("Space") {
            "↑↓ Space Enter Esc"
        } else {
            "↑↓ Enter Esc"
        };
        let foot = if width < 50 {
            compact.to_owned()
        } else {
            format!(
                "{}{}{}",
                if narrow { "" } else { "╰─ " },
                footer,
                if lines.len() > body_height {
                    " · PgUp/PgDn"
                } else {
                    ""
                }
            )
        };
        rendered.push(format!(
            "\x1b[2m{}\x1b[0m",
            wrap(&safe_text(&foot), width)
                .first()
                .cloned()
                .unwrap_or_default()
        ));
        let mut out = io::stderr().lock();
        for line in &rendered {
            write!(out, "{line}\r\n")?;
        }
        out.flush()?;
        self.occupied = rendered.len();
        Ok(())
    }

    fn page(&mut self, down: bool) {
        let step = usize::from(terminal_size().1).saturating_sub(4).max(1);
        self.scroll = if down {
            self.scroll.saturating_add(step)
        } else {
            self.scroll.saturating_sub(step)
        };
    }
    fn finish(&mut self, lines: &[String]) -> io::Result<()> {
        self.clear()?;
        let mut out = io::stderr().lock();
        let width = usize::from(terminal_size().0).saturating_sub(4).max(1);
        for line in lines {
            for part in wrap(&safe_text(line), width) {
                write!(out, "\x1b[32m{part}\x1b[0m\r\n")?;
            }
        }
        out.flush()
    }
}

#[derive(Default)]
struct Editor {
    text: String,
    cursor: usize,
}
impl Editor {
    fn insert(&mut self, value: &str) -> Result<(), &'static str> {
        if value.contains('\0') {
            return Err("不能输入 NUL");
        }
        let value = value.replace("\r\n", " ").replace(['\r', '\n'], " ");
        if self.text.chars().count() + value.chars().count() > 2048 {
            return Err("最多 2048 字符");
        }
        self.text.insert_str(self.cursor, &value);
        self.cursor += value.len();
        Ok(())
    }
    fn left(&mut self) {
        self.cursor = self.text[..self.cursor]
            .char_indices()
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
    }
    fn right(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }
    fn backspace(&mut self) {
        let end = self.cursor;
        self.left();
        self.text.drain(self.cursor..end);
    }
    fn delete(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.text.drain(self.cursor..self.cursor + c.len_utf8());
        }
    }
    fn display(&self) -> String {
        format!(
            "{}▏{}",
            &self.text[..self.cursor],
            &self.text[self.cursor..]
        )
    }
}

struct QuestionState {
    focus: usize,
    answer: AskAnswer,
    editor: Editor,
    editing: bool,
}
impl QuestionState {
    fn new(q: &AskQuestion) -> Self {
        Self {
            focus: q
                .recommended
                .as_ref()
                .and_then(|r| q.options.iter().position(|o| &o.id == r))
                .unwrap_or(0),
            answer: AskAnswer {
                question_id: q.id.clone(),
                selected: vec![],
                custom: None,
            },
            editor: Editor::default(),
            editing: q.options.is_empty(),
        }
    }
}

fn summary(r: &AskRequest, answers: &[AskAnswer]) -> Vec<String> {
    let mut lines = vec!["已补充".into()];
    for (q, a) in r.questions.iter().zip(answers) {
        let mut values = q
            .options
            .iter()
            .filter(|o| a.selected.contains(&o.id))
            .map(|o| o.label.clone())
            .collect::<Vec<_>>();
        if let Some(c) = &a.custom {
            values.push(c.clone());
        }
        lines.push(format!(
            "{}: {}",
            q.header.as_deref().unwrap_or(&q.question),
            values.join("；")
        ));
    }
    lines
}

fn cancellation(key: KeyEvent, empty: bool) -> Option<AskCancelReason> {
    if key.code == KeyCode::Esc {
        Some(AskCancelReason::Cancelled)
    } else if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        Some(AskCancelReason::Interrupted)
    } else if empty
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Char('d')
    {
        Some(AskCancelReason::Eof)
    } else {
        None
    }
}

fn ask_card(r: &AskRequest) -> io::Result<AskResult> {
    let _guard = TerminalGuard::enter()?;
    let mut card = Card::default();
    let mut states = r
        .questions
        .iter()
        .map(QuestionState::new)
        .collect::<Vec<_>>();
    let mut index = 0;
    let mut notice = String::new();
    let mut following_focus = true;
    loop {
        let q = &r.questions[index];
        let state = &mut states[index];
        let mut lines = vec![q.question.clone(), String::new()];
        let focus_line;
        if state.editing {
            lines.push("自定义回答:".into());
            let (cols, _) = terminal_size();
            let width = usize::from(cols)
                .saturating_sub(4)
                .min(88)
                .saturating_sub(if cols < 40 { 0 } else { 2 })
                .max(1);
            let editor_lines = wrap(&safe_text(&state.editor.display()), width);
            let before = wrap(&safe_text(&state.editor.text[..state.editor.cursor]), width);
            focus_line = lines.len() + before.len().saturating_sub(1);
            lines.extend(editor_lines);
        } else {
            let mut focused = 2;
            for (n, o) in q.options.iter().enumerate() {
                if n == state.focus {
                    focused = lines.len();
                }
                lines.push(format!(
                    "{} {}{}{}",
                    if n == state.focus { "›" } else { " " },
                    if q.multi {
                        if state.answer.selected.contains(&o.id) {
                            "[x] "
                        } else {
                            "[ ] "
                        }
                    } else {
                        ""
                    },
                    o.label,
                    if q.recommended.as_ref() == Some(&o.id) {
                        " · 推荐"
                    } else {
                        ""
                    }
                ));
                if n == state.focus {
                    if let Some(d) = &o.description {
                        lines.push(format!("  {d}"));
                    }
                }
            }
            if state.focus == q.options.len() {
                focused = lines.len();
            }
            lines.push(format!(
                "{} 自定义回答…{}",
                if state.focus == q.options.len() {
                    "›"
                } else {
                    " "
                },
                if state.answer.custom.is_some() {
                    " [已填]"
                } else {
                    ""
                }
            ));
            focus_line = focused;
        }
        if !notice.is_empty() {
            lines.push(notice.clone());
        }
        card.render(
            &format!(
                "需要补充{} · {} / {}",
                q.header
                    .as_ref()
                    .map(|h| format!(" · {h}"))
                    .unwrap_or_default(),
                index + 1,
                r.questions.len()
            ),
            &lines,
            if state.editing {
                "Enter 确定 · Shift-Tab 上一问 · Esc 取消"
            } else if q.multi {
                "↑↓ 选择 · Space 切换 · Enter 确定 · Esc 取消"
            } else {
                "↑↓ 选择 · Enter 确定 · Esc 取消"
            },
            following_focus.then_some(focus_line),
        )?;
        following_focus = false;
        match read_event()? {
            Event::Paste(value) if state.editing => {
                notice = state.editor.insert(&value).err().unwrap_or("").into();
                following_focus = true;
            }
            Event::Resize(_, _) => {
                following_focus = true;
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if let Some(reason) =
                    cancellation(key, !state.editing || state.editor.text.is_empty())
                {
                    card.finish(&["补充已取消".into()])?;
                    return Ok(cancelled(reason));
                }
                match key.code {
                    KeyCode::PageDown => card.page(true),
                    KeyCode::PageUp => card.page(false),
                    KeyCode::BackTab if index > 0 => {
                        index -= 1;
                        card.scroll = 0;
                        notice.clear();
                        following_focus = true;
                    }
                    KeyCode::Up if !state.editing => {
                        state.focus = (state.focus + q.options.len()) % (q.options.len() + 1);
                        following_focus = true;
                    }
                    KeyCode::Down if !state.editing => {
                        state.focus = (state.focus + 1) % (q.options.len() + 1);
                        following_focus = true;
                    }
                    KeyCode::Char(' ')
                        if !state.editing && q.multi && state.focus < q.options.len() =>
                    {
                        let id = &q.options[state.focus].id;
                        if state.answer.selected.contains(id) {
                            state.answer.selected.retain(|i| i != id);
                        } else {
                            state.answer.selected.push(id.clone());
                        }
                    }
                    KeyCode::Enter => {
                        if state.editing {
                            if state.editor.text.trim().is_empty() {
                                notice = "回答不能为空".into();
                                continue;
                            }
                            state.answer.custom = Some(state.editor.text.clone());
                            if q.multi {
                                state.editing = false;
                                state.focus = 0;
                                following_focus = true;
                                continue;
                            }
                            state.answer.selected.clear();
                        } else if state.focus == q.options.len() {
                            state.editing = true;
                            following_focus = true;
                            continue;
                        } else if !q.multi {
                            state.answer.selected = vec![q.options[state.focus].id.clone()];
                            state.answer.custom = None;
                        }
                        if validate_one(q, &state.answer).is_err() {
                            notice = "请至少选择一项或填写自定义回答".into();
                            continue;
                        }
                        if index + 1 == r.questions.len() {
                            let answers = states.into_iter().map(|s| s.answer).collect::<Vec<_>>();
                            card.finish(&summary(r, &answers))?;
                            return Ok(AskResult::Answered { answers });
                        }
                        index += 1;
                        card.scroll = 0;
                        notice.clear();
                        following_focus = true;
                    }
                    KeyCode::Left if state.editing => {
                        state.editor.left();
                        following_focus = true;
                    }
                    KeyCode::Right if state.editing => {
                        state.editor.right();
                        following_focus = true;
                    }
                    KeyCode::Home if state.editing => {
                        state.editor.cursor = 0;
                        following_focus = true;
                    }
                    KeyCode::End if state.editing => {
                        state.editor.cursor = state.editor.text.len();
                        following_focus = true;
                    }
                    KeyCode::Backspace if state.editing => {
                        state.editor.backspace();
                        following_focus = true;
                    }
                    KeyCode::Delete if state.editing => {
                        state.editor.delete();
                        following_focus = true;
                    }
                    KeyCode::Char(c)
                        if state.editing
                            && !key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        notice = state
                            .editor
                            .insert(&c.to_string())
                            .err()
                            .unwrap_or("")
                            .into();
                        following_focus = true;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn confirm_card(r: &ConfirmationRequest) -> io::Result<bool> {
    let _guard = TerminalGuard::enter()?;
    let mut card = Card::default();
    let mut allow = false;
    let mut details = false;
    loop {
        let mut lines = if details {
            vec![confirmation_details(r)]
        } else {
            let mut lines = vec![format!("工具: {}", r.tool_name), r.summary.clone()];
            if let Some(risk) = &r.risk_label {
                lines.push(format!("风险: {risk}"));
            }
            if let Some(reason) = &r.reason {
                lines.extend(reason.lines().take(3).map(str::to_owned));
            }
            lines.push(format!("{} 拒绝", if !allow { "›" } else { " " }));
            lines.push(format!("{} 仅允许本次", if allow { "›" } else { " " }));
            lines
        };
        if lines.is_empty() {
            lines.push(String::new());
        }
        let focus = (!details).then(|| lines.len() - if allow { 1 } else { 2 });
        card.render(
            "需要确认",
            &lines,
            if details {
                "Esc 返回 · PgUp/PgDn 浏览"
            } else {
                "↑↓ 选择 · Enter 确定 · y 允许 · n 拒绝 · d 详情"
            },
            focus,
        )?;
        match read_event()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => {
                if details && k.code == KeyCode::Esc {
                    details = false;
                    card.scroll = 0;
                    continue;
                }
                if cancellation(k, true).is_some() || k.code == KeyCode::Char('n') {
                    card.finish(&["已拒绝".into()])?;
                    return Ok(false);
                }
                match k.code {
                    KeyCode::Char('y') => {
                        card.finish(&["仅允许本次".into()])?;
                        return Ok(true);
                    }
                    KeyCode::Enter if !details => {
                        card.finish(&[if allow {
                            "仅允许本次".into()
                        } else {
                            "已拒绝".into()
                        }])?;
                        return Ok(allow);
                    }
                    KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right if !details => {
                        allow = !allow;
                    }
                    KeyCode::Char('d') => {
                        details = true;
                        card.scroll = 0;
                    }
                    KeyCode::PageDown => card.page(true),
                    KeyCode::PageUp => card.page(false),
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interaction_fallback_consumes_only_current_answers() {
        let r: AskRequest = serde_json::from_value(serde_json::json!({"questions":[{"id":"q","question":"选择","options":[{"id":"a","label":"甲"},{"id":"b","label":"乙"}]}]})).unwrap();
        let mut input = std::io::Cursor::new(b"\n9\n2\nnext\n".to_vec());
        let mut out = Vec::new();
        assert!(
            matches!(ask_plain(&r, &mut input, &mut out).unwrap(), AskResult::Answered { answers } if answers[0].selected == ["b"])
        );
        let mut line = String::new();
        input.read_line(&mut line).unwrap();
        assert_eq!(line, "next\n");
    }
    #[test]
    fn interaction_fallback_cancellation_discards_batch() {
        let r: AskRequest = serde_json::from_value(serde_json::json!({"questions":[{"id":"a","question":"一"},{"id":"b","question":"二"}]})).unwrap();
        let result = ask_plain(
            &r,
            &mut std::io::Cursor::new(b"answer\n/cancel\n"),
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(result, cancelled(AskCancelReason::Cancelled));
    }
    #[test]
    fn interaction_sanitizes_controls_and_wraps_unicode() {
        let s = safe_text("a\x1b]52;evil\x07\u{202e}中\n文");
        assert!(!s.contains('\x1b'));
        assert!(!s.contains('\u{202e}'));
        assert!(s.contains('\n'));
        assert_eq!(wrap("中文abc", 4), vec!["中文", "abc"]);
    }
    #[test]
    fn interaction_text_editing_is_utf8_safe() {
        let mut e = Editor::default();
        e.insert("中文").unwrap();
        e.left();
        e.insert("路径").unwrap();
        e.backspace();
        assert_eq!(e.text, "中路文");
        e.delete();
        assert_eq!(e.text, "中路");
        e.insert("\n粘贴\r\n").unwrap();
        assert!(!e.text.contains('\n'));
        assert!(e.insert("\0").is_err());
        assert!(e.insert(&"x".repeat(2049)).is_err());
    }
    #[test]
    fn interaction_confirmation_defaults_to_denial() {
        let r = ConfirmationRequest {
            tool_name: "shell".into(),
            summary: "delete".into(),
            details: "real command".into(),
            reason: None,
            missing_evidence: vec![],
            risk_label: None,
            preview: None,
        };
        assert!(!confirm_plain(&r, &mut std::io::Cursor::new(b"\n"), &mut Vec::new()).unwrap());
        assert!(confirm_plain(&r, &mut std::io::Cursor::new(b" Y \n"), &mut Vec::new()).unwrap());
    }
    #[test]
    fn interaction_fallback_multiselect_custom_and_eof() {
        let r: AskRequest = serde_json::from_value(serde_json::json!({"questions":[{"id":"q","question":"选择","multi":true,"options":[{"id":"a","label":"甲"},{"id":"b","label":"乙"}]}]})).unwrap();
        let result = ask_plain(
            &r,
            &mut std::io::Cursor::new("1,0\n中文路径\n"),
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(
            result,
            AskResult::Answered {
                answers: vec![AskAnswer {
                    question_id: "q".into(),
                    selected: vec!["a".into()],
                    custom: Some("中文路径".into())
                }]
            }
        );
        assert_eq!(
            ask_plain(&r, &mut std::io::Cursor::new(b"0\n"), &mut Vec::new()).unwrap(),
            cancelled(AskCancelReason::Eof)
        );
    }
    #[test]
    fn interaction_recommended_focus_is_not_an_answer() {
        let q: AskQuestion = serde_json::from_value(serde_json::json!({"id":"q","question":"选择","recommended":"b","options":[{"id":"a","label":"甲"},{"id":"b","label":"乙"}]})).unwrap();
        let state = QuestionState::new(&q);
        assert_eq!(state.focus, 1);
        assert!(state.answer.selected.is_empty());
        assert!(validate_one(&q, &state.answer).is_err());
        assert_eq!(
            cancellation(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                true
            ),
            Some(AskCancelReason::Interrupted)
        );
        assert_eq!(
            cancellation(
                KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
                false
            ),
            None
        );
        assert_eq!(
            cancellation(
                KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
                true
            ),
            Some(AskCancelReason::Eof)
        );
    }
}
