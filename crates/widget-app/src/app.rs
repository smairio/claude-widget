//! The eframe UI: an opaque, frameless status card. Skeleton scope = working/idle.

use std::sync::mpsc::Receiver;

use widget_core::{AggregateState, ParsedEvent, Roster};

pub struct WidgetApp {
    roster: Roster,
    rx: Receiver<ParsedEvent>,
}

impl WidgetApp {
    pub fn new(rx: Receiver<ParsedEvent>) -> Self {
        Self { roster: Roster::new(), rx }
    }
}

fn presentation(state: AggregateState) -> (&'static str, egui::Color32) {
    match state {
        AggregateState::NoSessions => ("no sessions", egui::Color32::from_gray(95)),
        AggregateState::Idle => ("idle", egui::Color32::from_gray(160)),
        AggregateState::Working => ("working", egui::Color32::from_rgb(80, 140, 240)),
        AggregateState::WaitingForInput => ("needs you", egui::Color32::from_rgb(240, 95, 90)),
        AggregateState::RateLimited => ("rate limited", egui::Color32::from_rgb(240, 95, 90)),
    }
}

impl eframe::App for WidgetApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drain any hook events delivered since the last frame.
        while let Ok(ev) = self.rx.try_recv() {
            self.roster.apply(&ev);
        }

        let (label, accent) = presentation(self.roster.aggregate());
        let bg = egui::Color32::from_rgb(18, 18, 22);

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(bg).inner_margin(16.0))
            .show(ctx, |ui| {
                // Whole-card drag: frameless windows need an explicit drag handle.
                let drag = ui.interact(
                    ui.max_rect(),
                    egui::Id::new("card-drag"),
                    egui::Sense::drag(),
                );
                if drag.dragged() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }

                ui.label(
                    egui::RichText::new("Claude")
                        .size(14.0)
                        .color(egui::Color32::from_gray(210)),
                );
                ui.add_space(8.0);
                ui.label(egui::RichText::new(format!("● {label}")).size(24.0).color(accent));
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(format!("{} session(s)", self.roster.len()))
                        .size(12.0)
                        .color(egui::Color32::from_gray(140)),
                );
            });
    }
}
