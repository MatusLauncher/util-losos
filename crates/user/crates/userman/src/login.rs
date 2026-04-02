//! Ratatui-based interactive login screen for the `login` executable symlink.

use std::io::{self, Stdout, Write};

use base64::{Engine, prelude::BASE64_STANDARD};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use miette::IntoDiagnostic;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph},
};

use crate::{
    crypto::unlock_home,
    daemon::{TwoFA, UserAPI, UserSchema},
    twofa::{validate_totp, verify_fido2},
};

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

#[derive(PartialEq, Eq)]
enum Screen {
    Credentials,
    TotpCode,
    SecondPassword,
    Fido2Pin,
    /// Screen is painted, then `verify_fido2` blocks the thread.
    Fido2Waiting,
    Done,
}

enum CredField {
    Username,
    Password,
}

struct LoginScreen {
    screen: Screen,
    username: String,
    password: String,
    cred_focus: CredField,
    totp_input: String,
    second_pass_input: String,
    fido2_pin: String,
    error_msg: Option<String>,
    /// Populated after primary-auth succeeds; consumed by 2FA and LUKS paths.
    authenticated_user: Option<UserSchema>,
}

impl Default for LoginScreen {
    fn default() -> Self {
        Self {
            screen: Screen::Credentials,
            username: String::new(),
            password: String::new(),
            cred_focus: CredField::Username,
            totp_input: String::new(),
            second_pass_input: String::new(),
            fido2_pin: String::new(),
            error_msg: None,
            authenticated_user: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Terminal RAII guard — restores the terminal on drop (including panics)
// ---------------------------------------------------------------------------

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    fn enter() -> miette::Result<Self> {
        enable_raw_mode().into_diagnostic()?;
        execute!(io::stdout(), EnterAlternateScreen).into_diagnostic()?;
        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::new(backend).into_diagnostic()?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn run_login_screen(daemon_api: &UserAPI) -> miette::Result<()> {
    // Ensure the terminal is restored even on a panic.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original_hook(info);
    }));

    let mut guard = TerminalGuard::enter()?;
    let mut app = LoginScreen::default();

    loop {
        guard.terminal.draw(|f| ui(f, &app)).into_diagnostic()?;
        if app.screen == Screen::Done {
            break;
        }
        if let Event::Key(k) = event::read().into_diagnostic()? {
            handle_key(&mut app, k, &mut guard.terminal, daemon_api)?;
        }
    }

    // LUKS unlock runs after the TUI exits cleanly.
    if let Some(user) = &app.authenticated_user
        && user.encryption()
        && let Some(device) = user.luks_device()
    {
        unlock_home(device, &app.password, &app.username)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------------

fn handle_key(
    app: &mut LoginScreen,
    key: crossterm::event::KeyEvent,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    daemon_api: &UserAPI,
) -> miette::Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.screen = Screen::Done;
        return Ok(());
    }
    match app.screen {
        Screen::Credentials    => handle_credentials(app, key, daemon_api)?,
        Screen::TotpCode       => handle_totp(app, key)?,
        Screen::SecondPassword => handle_second_password(app, key)?,
        Screen::Fido2Pin       => handle_fido2_pin(app, key, terminal)?,
        Screen::Fido2Waiting | Screen::Done => {}
    }
    Ok(())
}

fn handle_credentials(
    app: &mut LoginScreen,
    key: crossterm::event::KeyEvent,
    daemon_api: &UserAPI,
) -> miette::Result<()> {
    match key.code {
        KeyCode::Tab => {
            app.cred_focus = match app.cred_focus {
                CredField::Username => CredField::Password,
                CredField::Password => CredField::Username,
            };
        }
        KeyCode::Backspace => match app.cred_focus {
            CredField::Username => { app.username.pop(); }
            CredField::Password => { app.password.pop(); }
        },
        KeyCode::Enter => {
            app.error_msg = None;

            let user = match daemon_api.user(&app.username) {
                Ok(u) => u,
                Err(_) => {
                    app.error_msg = Some("Invalid username or password.".into());
                    app.password.clear();
                    return Ok(());
                }
            };

            let stored = BASE64_STANDARD
                .decode(user.pass())
                .ok()
                .and_then(|b| String::from_utf8(b).ok());
            if stored.as_deref() != Some(app.password.as_str()) {
                app.error_msg = Some("Invalid username or password.".into());
                app.password.clear();
                return Ok(());
            }

            if user.locked_out() {
                app.error_msg = Some("Account is locked out.".into());
                app.password.clear();
                return Ok(());
            }

            unsafe { std::env::set_var("USER", &app.username) };

            app.screen = match user.twofa() {
                Some(TwoFA::TOTP)     => Screen::TotpCode,
                Some(TwoFA::Password) => Screen::SecondPassword,
                Some(TwoFA::Passkey)  => Screen::Fido2Pin,
                None                  => Screen::Done,
            };
            app.authenticated_user = Some(user);
        }
        KeyCode::Char(c) => match app.cred_focus {
            CredField::Username => app.username.push(c),
            CredField::Password => app.password.push(c),
        },
        _ => {}
    }
    Ok(())
}

fn handle_totp(app: &mut LoginScreen, key: crossterm::event::KeyEvent) -> miette::Result<()> {
    match key.code {
        KeyCode::Esc => {
            app.totp_input.clear();
            app.error_msg = None;
            app.screen = Screen::Credentials;
        }
        KeyCode::Backspace => { app.totp_input.pop(); }
        KeyCode::Enter => {
            app.error_msg = None;
            let user = app
                .authenticated_user
                .as_ref()
                .expect("user is set before entering TotpCode screen");
            let secret = match user.totp_secret() {
                Some(s) => s,
                None => {
                    app.error_msg = Some("No TOTP secret configured.".into());
                    return Ok(());
                }
            };
            match validate_totp(secret, &app.totp_input)? {
                true  => { app.totp_input.clear(); app.screen = Screen::Done; }
                false => {
                    app.error_msg = Some("Invalid TOTP code, try again.".into());
                    app.totp_input.clear();
                }
            }
        }
        KeyCode::Char(c) if c.is_ascii_digit() && app.totp_input.len() < 6 => {
            app.totp_input.push(c);
        }
        _ => {}
    }
    Ok(())
}

fn handle_second_password(
    app: &mut LoginScreen,
    key: crossterm::event::KeyEvent,
) -> miette::Result<()> {
    match key.code {
        KeyCode::Esc => {
            app.second_pass_input.clear();
            app.error_msg = None;
            app.screen = Screen::Credentials;
        }
        KeyCode::Backspace => { app.second_pass_input.pop(); }
        KeyCode::Enter => {
            app.error_msg = None;
            let user = app
                .authenticated_user
                .as_ref()
                .expect("user is set before entering SecondPassword screen");
            let stored = match user.second_pass() {
                Some(s) => s,
                None => {
                    app.error_msg = Some("No second password configured.".into());
                    return Ok(());
                }
            };
            if app.second_pass_input == stored {
                app.second_pass_input.clear();
                app.screen = Screen::Done;
            } else {
                app.error_msg = Some("Wrong second password, try again.".into());
                app.second_pass_input.clear();
            }
        }
        KeyCode::Char(c) => app.second_pass_input.push(c),
        _ => {}
    }
    Ok(())
}

fn handle_fido2_pin(
    app: &mut LoginScreen,
    key: crossterm::event::KeyEvent,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
) -> miette::Result<()> {
    match key.code {
        KeyCode::Esc => {
            app.fido2_pin.clear();
            app.error_msg = None;
            app.screen = Screen::Credentials;
        }
        KeyCode::Backspace => { app.fido2_pin.pop(); }
        KeyCode::Enter => {
            app.error_msg = None;
            // Paint the waiting screen and flush before blocking on verify_fido2.
            app.screen = Screen::Fido2Waiting;
            terminal.draw(|f| ui(f, app)).into_diagnostic()?;
            terminal.backend_mut().flush().into_diagnostic()?;

            let user = app
                .authenticated_user
                .as_ref()
                .expect("user is set before entering Fido2Pin screen");
            let (cred_id, pubkey_der, pubkey_type) = match (
                user.fido2_credential_id(),
                user.fido2_public_key_der(),
                user.fido2_public_key_type(),
            ) {
                (Some(c), Some(k), t) => (c, k, t.unwrap_or(0)),
                _ => {
                    app.error_msg = Some("FIDO2 credential not fully configured.".into());
                    app.fido2_pin.clear();
                    app.screen = Screen::Fido2Pin;
                    return Ok(());
                }
            };
            let pin_opt: Option<&str> =
                if app.fido2_pin.is_empty() { None } else { Some(&app.fido2_pin) };

            match verify_fido2(cred_id, pubkey_der, pubkey_type, pin_opt)? {
                true => { app.fido2_pin.clear(); app.screen = Screen::Done; }
                false => {
                    app.error_msg = Some("FIDO2 verification failed, try again.".into());
                    app.fido2_pin.clear();
                    app.screen = Screen::Fido2Pin;
                }
            }
        }
        KeyCode::Char(c) => app.fido2_pin.push(c),
        _ => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// UI rendering
// ---------------------------------------------------------------------------

fn ui(f: &mut Frame, app: &LoginScreen) {
    let area = centered_rect(62, 60, f.area());
    match app.screen {
        Screen::Credentials    => draw_credentials(f, area, app),
        Screen::TotpCode       => draw_single_input(
            f, area,
            " Two-Factor Authentication — TOTP ",
            "TOTP code (6 digits)",
            &app.totp_input,
            false,
            app.error_msg.as_deref(),
            "Enter: verify   Esc: back   Ctrl-C: quit",
        ),
        Screen::SecondPassword => draw_single_input(
            f, area,
            " Two-Factor Authentication — Password ",
            "Second password",
            &app.second_pass_input,
            true,
            app.error_msg.as_deref(),
            "Enter: verify   Esc: back   Ctrl-C: quit",
        ),
        Screen::Fido2Pin       => draw_single_input(
            f, area,
            " Two-Factor Authentication — FIDO2 ",
            "PIN (leave empty if not required)",
            &app.fido2_pin,
            true,
            app.error_msg.as_deref(),
            "Enter: confirm and wait for key tap   Esc: back",
        ),
        Screen::Fido2Waiting   => draw_message(
            f, area,
            " FIDO2 ",
            "Please touch your FIDO2 key when it blinks...",
        ),
        Screen::Done           => draw_message(
            f, area,
            " LosOS ",
            "Login successful.",
        ),
    }
}

fn draw_credentials(f: &mut Frame, area: Rect, app: &LoginScreen) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .title(" LosOS Login ")
        .title_style(Style::default().add_modifier(Modifier::BOLD));
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    // Layout: top-pad | username(3) | gap | password(3) | gap | error | fill | help | bot-pad
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    let user_focused = matches!(app.cred_focus, CredField::Username);

    // Username field
    let user_block = Block::default()
        .borders(Borders::ALL)
        .title(" Login ")
        .border_style(focused_style(user_focused));
    let user_inner = user_block.inner(chunks[1]);
    f.render_widget(user_block, chunks[1]);
    f.render_widget(Paragraph::new(app.username.as_str()), user_inner);

    // Password field
    let pass_block = Block::default()
        .borders(Borders::ALL)
        .title(" Password ")
        .border_style(focused_style(!user_focused));
    let pass_inner = pass_block.inner(chunks[3]);
    f.render_widget(pass_block, chunks[3]);
    f.render_widget(Paragraph::new("*".repeat(app.password.len())), pass_inner);

    // Error message
    if let Some(err) = &app.error_msg {
        f.render_widget(
            Paragraph::new(err.as_str()).style(Style::default().fg(Color::Red)),
            chunks[5],
        );
    }

    // Help bar
    f.render_widget(
        Paragraph::new(" Tab: switch field   Enter: submit   Ctrl-C: quit")
            .style(Style::default().fg(Color::DarkGray)),
        chunks[7],
    );

    // Text cursor
    if user_focused {
        let cx = (user_inner.x + app.username.len() as u16)
            .min(user_inner.right().saturating_sub(1));
        f.set_cursor_position((cx, user_inner.y));
    } else {
        let cx = (pass_inner.x + app.password.len() as u16)
            .min(pass_inner.right().saturating_sub(1));
        f.set_cursor_position((cx, pass_inner.y));
    }
}

/// Shared layout used by TOTP, SecondPassword, and Fido2Pin screens.
#[allow(clippy::too_many_arguments)]
fn draw_single_input(
    f: &mut Frame,
    area: Rect,
    title: &str,
    field_label: &str,
    input: &str,
    mask: bool,
    error: Option<&str>,
    help: &str,
) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_style(Style::default().add_modifier(Modifier::BOLD));
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let chunks = Layout::vertical([
        Constraint::Length(1), // top pad
        Constraint::Length(1), // field label
        Constraint::Length(3), // input box
        Constraint::Length(1), // gap
        Constraint::Length(1), // error
        Constraint::Min(0),    // fill
        Constraint::Length(1), // help
        Constraint::Length(1), // bot pad
    ])
    .split(inner);

    f.render_widget(Paragraph::new(field_label), chunks[1]);

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    let input_inner = input_block.inner(chunks[2]);
    f.render_widget(input_block, chunks[2]);
    let display = if mask { "*".repeat(input.len()) } else { input.to_string() };
    f.render_widget(Paragraph::new(display), input_inner);

    if let Some(err) = error {
        f.render_widget(
            Paragraph::new(err).style(Style::default().fg(Color::Red)),
            chunks[4],
        );
    }

    f.render_widget(
        Paragraph::new(help).style(Style::default().fg(Color::DarkGray)),
        chunks[6],
    );

    // Cursor at end of input
    let cx = (input_inner.x + input.len() as u16).min(input_inner.right().saturating_sub(1));
    f.set_cursor_position((cx, input_inner.y));
}

fn draw_message(f: &mut Frame, area: Rect, title: &str, msg: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_style(Style::default().add_modifier(Modifier::BOLD));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(inner);

    f.render_widget(
        Paragraph::new(msg).style(Style::default().add_modifier(Modifier::BOLD)),
        chunks[1],
    );
}

fn focused_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let margin_y = (100u16.saturating_sub(percent_y)) / 2;
    let margin_x = (100u16.saturating_sub(percent_x)) / 2;
    let vert = Layout::vertical([
        Constraint::Percentage(margin_y),
        Constraint::Percentage(percent_y),
        Constraint::Percentage(margin_y),
    ])
    .split(r);
    Layout::horizontal([
        Constraint::Percentage(margin_x),
        Constraint::Percentage(percent_x),
        Constraint::Percentage(margin_x),
    ])
    .split(vert[1])[1]
}
