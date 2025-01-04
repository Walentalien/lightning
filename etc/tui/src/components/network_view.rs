use anyhow::Result;
use indoc::indoc;
use ratatui::prelude::{Constraint, Layout, Rect, Alignment};
use ratatui::widgets::Paragraph;
use ratatui::widgets::Borders;
//use ratatui::style::Style;
//use ratatui::style::Color;
use tokio::sync::mpsc::UnboundedSender;
use ratatui::widgets::Block;
//use ratatui::text::Text;
use super::{Component, Frame};
use crate::action::Action;
use crate::config::Config;
use crate::state::State;
//use ratatui::widgets::ListItem;
//use ratatui::widgets::List;
use ratatui::widgets::{List, ListItem};
use ratatui::text::{Text, Line, Span};
use ratatui::style::{Style, Color};
pub struct NetworkView {
    command_tx: Option<UnboundedSender<Action>>,
    title: String,
    config: Config,
    current_epoch: u64,
    ethereum_address: String,
}

impl NetworkView {


    // TODO: Update
    pub fn new() -> Self {
        Self {
            command_tx: None,
            title: String::default(),
            config: Config::default(),
            current_epoch: 0,
            ethereum_address: String::default(),
        }
    }

    pub fn set_curr_epoch(&mut self, epoch: u64) {
        self.current_epoch =  epoch;
    }
    pub fn set_ethereum_address(&mut self, ethereum_address: String) { self.ethereum_address = ethereum_address; }
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
            Constraint::Max(500),
            Constraint::Fill(1),
        ])
        .split(vchunks[1]);

        let logo = Layout::horizontal([Constraint::Length(12), Constraint::Fill(1)]).split(hchunks[1]);
        let title = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).split(logo[1]);
        let metrics = List::new(vec![
            ListItem::new(
                Text::from(Line::from(vec![
                    Span::raw("This is the current epoch: "),
                    Span::styled(
                        format!("{}", self.current_epoch),
                        Style::default().fg(Color::Green),
                    ),
                ])),
            ),
            ListItem::new(
                Text::from(Line::from(vec![
                    Span::raw("This is the current Ethereum address: "),
                    Span::styled(
                        format!("{}", self.ethereum_address),
                        Style::default().fg(Color::Cyan),
                    ),
                ])),
            ),
        ])
            .block(
                Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green))
                    .title_alignment(Alignment::Center)
                    .title("Metrics")
            );
        //f.render_widget(Paragraph::new(format!("This is the current epoch:{}\nThis is the current ethereum address:{}",self.current_epoch,self.ethereum_address)).alignment(Alignment::Left), title[0]);
        f.render_widget(metrics, title[0]);
        Ok(())
    }

}
