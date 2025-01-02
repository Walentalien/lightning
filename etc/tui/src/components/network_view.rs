use anyhow::Result;
use indoc::indoc;
use ratatui::prelude::{Constraint, Layout, Rect};
use ratatui::widgets::Paragraph;
use tokio::sync::mpsc::UnboundedSender;

use super::{Component, Frame};
use crate::action::Action;
use crate::config::Config;
use crate::state::State;

pub struct NetworkView {
    command_tx: Option<UnboundedSender<Action>>,
    title: String,
    config: Config,
    current_epoch: u64,
}

impl NetworkView {


    // TODO: Update
    pub fn new() -> Self {


        Self {
            command_tx: None,
            title: String::default(),
            config: Config::default(),
            current_epoch: 0,
        }
    }

    pub fn set_curr_epoch(&mut self, epoch: u64) {
        self.current_epoch =  epoch;
    }
}

impl Component for NetworkView {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> Result<()> {
        self.command_tx = Some(tx);
        Ok(())
    }

    fn register_config_handler(&mut self, config: Config) -> Result<()> {
        self.config = config;
        Ok(())
    }

    //TODO: Update draw
    fn draw(&mut self, f: &mut Frame<'_>, area: Rect) -> Result<()> {
        let vchunks = Layout::vertical([
            Constraint::Percentage(30),
            Constraint::Percentage(40),
            Constraint::Percentage(30),
        ])
        .split(area);

        let hchunks = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Max(42),
            Constraint::Fill(1),
        ])
        .split(vchunks[1]);

        let logo =
            Layout::horizontal([Constraint::Length(12), Constraint::Fill(1)]).split(hchunks[1]);



        let title = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).split(logo[1]);
        f.render_widget(Paragraph::new(format!("This is the current epoch:{}",self.current_epoch)).centered(), title[1]);

        Ok(())
    }

}
