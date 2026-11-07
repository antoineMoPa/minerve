//! `minerve --serve`: the conversation without a terminal, driven over the control socket.
//!
//! minerve-gui starts this and sends `submit`, `cancel` and `new_session`; it answers with the
//! events listed in `control_socket` (`transcript`, `status`, `notice`, `usage`, and the usual ones).
//! Ends when the parent closes the socket.

use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};

use crate::boot;
use crate::pro_tips;
use crate::control_socket::{ControlSocket, MinerveEvent, ParentCommand};
use crate::minerve::Minerve;
use crate::openrouter::OpenRouterConnection;
use crate::serve_commands::Extras;
use crate::session::SessionStart;
use crate::setup;
use crate::ui::UiEvent;

pub fn run(
    connection: OpenRouterConnection,
    start: SessionStart,
    extra_system_prompts: &[String],
    control: ControlSocket,
) {
    let (tx, rx) = mpsc::channel();
    let control = Arc::new(control);
    let commands_tx = tx.clone();
    let closed_tx = tx.clone();
    control.read_commands_in_background(
        move |command| {
            let _ = commands_tx.send(UiEvent::ParentCommand(command));
        },
        move || {
            let _ = closed_tx.send(UiEvent::ParentClosed);
        },
    );
    let needs_setup = setup::needs_setup(&connection.api_keys);
    let minerve = Arc::new(Minerve::new(connection, start, extra_system_prompts, Some(control.clone())));
    minerve.fetch_context_window_in_background();

    send_transcript(&control, &minerve);
    if needs_setup {
        control.send(&MinerveEvent::Notice {
            notice: "No API key set. Run `minerve --setup` in a terminal, then restart minerve-gui.",
        });
    }

    let mut extras = Extras::new();
    while let Ok(event) = rx.recv() {
        match event {
            UiEvent::Transcript(messages) => {
                control.send(&MinerveEvent::Transcript { messages: &messages });
                send_usage(&control, &minerve);
            }
            UiEvent::Status(status) => {
                control.send(&MinerveEvent::Status { status: &status });
                send_usage(&control, &minerve);
            }
            UiEvent::Notice(notice) => control.send(&MinerveEvent::Notice { notice: &notice }),
            UiEvent::ModelList(provider, result) => extras.on_model_list(provider, result, &minerve, &control),
            UiEvent::ParentCommand(command) => match command {
                ParentCommand::Submit { text, images } => {
                    if minerve.request_in_flight.load(Ordering::SeqCst) {
                        control.send(&MinerveEvent::Notice { notice: "busy: a turn is running" });
                    } else {
                        minerve.clone().chat_with_arc(text, images, tx.clone(), false);
                    }
                }
                ParentCommand::Cancel {} => minerve.cancel_turn(),
                ParentCommand::NewSession {} => {
                    if minerve.request_in_flight.load(Ordering::SeqCst) {
                        control.send(&MinerveEvent::Notice { notice: "busy: a turn is running" });
                    } else {
                        minerve.start_new_session();
                        send_transcript(&control, &minerve);
                    }
                }
                ParentCommand::GetHistory {} => minerve.send_history_to_control(),
                ParentCommand::GetIntro {} => control.send(&MinerveEvent::Intro {
                    fps: boot::FPS as u32,
                    caption: boot::CAPTION,
                    frames: &boot::frames(),
                    tips: pro_tips::PRO_TIPS,
                }),
                // The GUI keeps its own view and input box.
                ParentCommand::Scroll { .. } | ParentCommand::ScrollBy { .. } | ParentCommand::InsertText { .. } => {}
                other => {
                    extras.handle(other, &minerve, &control, &tx);
                }
            },
            UiEvent::ParentClosed => {
                // No save: `end_turn` already saved every finished turn, and saving now could keep a
                // half-done tool call or an empty chat.
                minerve.cancel_turn();
                // Lets go of Slack's lock if the window started the gateway.
                crate::message_apps_runner::stop();
                return;
            }
        }
    }
}

fn send_transcript(control: &ControlSocket, minerve: &Minerve) {
    control.send(&MinerveEvent::Transcript { messages: &minerve.ui_messages() });
    send_usage(control, minerve);
}

fn send_usage(control: &ControlSocket, minerve: &Minerve) {
    control.send(&MinerveEvent::Usage { summary: &minerve.token_counter.format_usage_summary() });
}
