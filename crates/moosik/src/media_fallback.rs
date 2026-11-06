//! macOS's opt-in Apple Music fallback. The OS registration is the source of truth, so
//! disabling background activity in System Settings is reflected here too. No profile flag
//! silently re-enables it at startup. All ServiceManagement work runs off the UI thread.

use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Off,
    On,
    Approval,
}

fn parse_status(output: &str) -> Result<Status, String> {
    match output.trim() {
        "off" => Ok(Status::Off),
        "on" => Ok(Status::On),
        "approval" => Ok(Status::Approval),
        _ => Err("macOS returned an unrecognised background activity status.".into()),
    }
}

fn command(argument: &str) -> Result<Status, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let helper = executable
        .parent()
        .ok_or("Could not locate Moosik.")?
        .join("moosik-media-helper");
    if !helper.is_file() {
        return Err("Install the latest Moosik app to use this setting.".into());
    }
    let output = std::process::Command::new(helper)
        .arg(argument)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if error.is_empty() {
            "macOS could not change background activity. Try again.".into()
        } else {
            error
        });
    }
    parse_status(&String::from_utf8_lossy(&output.stdout))
}

#[derive(Default)]
pub struct State {
    pub status: Option<Status>,
    pub error: Option<String>,
    pending: Option<Receiver<Result<Status, String>>>,
    checked: Option<Instant>,
    changing: bool,
}

impl State {
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    pub fn refresh(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = &self.pending {
            match receiver.try_recv() {
                Ok(result) => {
                    match result {
                        Ok(status) => {
                            self.status = Some(status);
                            if self.changing {
                                self.error = None;
                            }
                        }
                        Err(error) => self.error = Some(error),
                    }
                    self.pending = None;
                    self.checked = Some(Instant::now());
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.checked = Some(Instant::now());
                    self.error = Some("Could not read macOS background activity.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if !self.busy()
            && self
                .checked
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(2))
        {
            self.start(ctx, "--status");
        }
        ctx.request_repaint_after(Duration::from_secs(2));
    }

    pub fn start(&mut self, ctx: &egui::Context, argument: &'static str) {
        if self.busy() {
            return;
        }
        self.changing = argument != "--status";
        let (sender, receiver) = mpsc::channel();
        self.pending = Some(receiver);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(command(argument));
            ctx.request_repaint();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_confirmed_registration_is_active() {
        assert_eq!(parse_status("on\n"), Ok(Status::On));
        assert_eq!(parse_status("off\n"), Ok(Status::Off));
        assert_eq!(parse_status("approval\n"), Ok(Status::Approval));
        assert!(parse_status("unknown").is_err());
        assert!(parse_status("").is_err());
    }
}
