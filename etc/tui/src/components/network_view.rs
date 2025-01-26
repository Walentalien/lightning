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
use crate::state::{State,StakeInfo};
//use ratatui::widgets::ListItem;
//use ratatui::widgets::List;
use ratatui::widgets::{List, ListItem};
use ratatui::text::{Text, Line, Span};
use ratatui::style::{Style, Color};


/*
+-+   +-+   +-+
|K|   |M|   |B|
+-+   +-+   +-+

|__/     |\/|    |__)
|  \     |  |    |__)

▗▖ ▗▖     ▗▖  ▗▖     ▗▄▄▖
▐▌▗▞▘     ▐▛▚▞▜▌     ▐▌ ▐▌
▐▛▚▖      ▐▌  ▐▌     ▐▛▀▚▖
▐▌ ▐▌     ▐▌  ▐▌     ▐▙▄▞▘


▗▖ ▗▖
▐▌▗▞▘
▐▛▚▖
▐▌ ▐▌



▗▄▄▖
▐▌ ▐▌
▐▛▀▚▖
▐▙▄▞▘
 */
/*
let b_letter = indoc! {"
   ▗▖ ▗▖
   ▐▌▗▞▘
   ▐▛▚▖
   ▐▌ ▐▌
"};

let k_letter = indoc! {"
   ▗▖  ▗▖
   ▐▛▚▞▜▌
   ▐▌  ▐▌
   ▐▌  ▐▌
"};

let m_letter = indoc! {"
   ▗▄▄▖
   ▐▌ ▐▌
   ▐▛▀▚▖
   ▐▙▄▞▘
"};*/


const BIG_NUMBERS: [&str; 10] = [
    indoc! {"
       ▄▄▄
       █ █
       █▄█
    "}, // 0
    indoc! {"
         ▄
        ▀█
         █
    "}, // 1
    indoc! {"
       ▄▄▄
       ▄▄█
       █▄▄
    "}, // 2
    indoc! {"
       ▄▄▄
        ▄█
       ▄▄█
    "}, // 3
    indoc! {"
       ▄ ▄
       █▄█
         █
    "}, // 4
    indoc! {"
       ▄▄▄
       █▄▄
       ▄▄█
    "}, // 5
    indoc! {"
       ▄▄▄
       █▄▄
       █▄█
    "}, // 6
    indoc! {"
       ▄▄▄
         █
         █
    "}, // 7
    indoc! {"
       ▄▄▄
       █▄█
       █▄█
    "}, // 8
    indoc! {"
       ▄▄▄
       █▄█
         █
    "}, // 9
];

fn number_to_big(num: u128) -> String {
    let digits: Vec<char> = num.to_string().chars().collect();
    let mut lines = vec![String::new(), String::new(), String::new()];

    for digit in digits {
        if let Some(digit_index) = digit.to_digit(10) {
            let ascii_digit = BIG_NUMBERS[digit_index as usize];
            let ascii_lines: Vec<&str> = ascii_digit.lines().collect();

            for (i, line) in ascii_lines.iter().enumerate() {
                lines[i].push_str(line);
                lines[i].push(' '); // Add space between digits
            }
        }
    }

    lines.join("\n")
}

pub struct NetworkView {
    command_tx: Option<UnboundedSender<Action>>,
    title: String,
    config: Config,
    current_epoch: u64,
    ethereum_address: String,
    public_key: String,
    consensus_key: String,
    participation: String,
    reputation: String,
    uptime: String,
    stake: StakeInfo,
    committee_members: Vec<String>,
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
            public_key: String::default(),
            consensus_key: String::default(),
            participation: String::default(),
            reputation: String::default(),
            uptime: String::default(),
            stake: StakeInfo{
                staked: "error occurred while writing".to_string(),
                stake_locked_until: 0,
                locked: "error occurred while writing".to_string(),
                locked_until: 0,
            },
            committee_members: Vec::new(),
        }
    }

    pub fn set_curr_epoch(&mut self, epoch: u64) {
        self.current_epoch =  epoch;
    }
    pub fn set_ethereum_address(&mut self, ethereum_address: String) { self.ethereum_address = ethereum_address; }

    pub fn set_node_public_key(&mut self, public_key: String) { self.public_key = public_key; }
    pub fn set_consensus_key(&mut self, consensus_key: String) { self.consensus_key = consensus_key; }

    pub fn set_staked(&mut self, stake: String) {self.stake.staked = stake;}

    pub fn set_stake_locked_until(&mut self, stake_locked_until: u64) {self.stake.stake_locked_until = stake_locked_until;}

    pub fn set_get_locked(&mut self, locked: String){ self.stake.locked = locked;}

    pub fn set_get_locked_until(&mut self, locked_until: u64) {self.stake.locked_until = locked_until;}

    pub fn set_participation(&mut self, participation: String) { self.participation = participation; }

    pub fn set_reputation(&mut self, reputation:String) { self.reputation = reputation; }

    pub fn set_uptime(&mut self, uptime: String) { self.uptime = uptime; }
    
    pub fn set_committee_members(&mut self, committee_members: Vec<String>) {
        self.committee_members = committee_members;
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

    fn draw(&mut self, f: &mut Frame<'_>, area: Rect) -> Result<()> {
        // Main layout: Vertical split for rows
        let vchunks = Layout::vertical([
            Constraint::Length(7),  // First row
            Constraint::Length(7),  // Second row
            Constraint::Min(10),    // Third row
        ])
            .split(area);

        // First row: Horizontal split for epoch, locked, and reputation
        let row1 = Layout::horizontal([
            Constraint::Percentage(25), // Epoch
            Constraint::Percentage(25), // Locked
            Constraint::Percentage(25), // Reputation
            Constraint::Percentage(25), // Uptime
        ])
            .split(vchunks[0]);

        // Second row: Horizontal split for staked
        let row2 = Layout::horizontal([
            Constraint::Percentage(100), // Staked
        ])
            .split(vchunks[1]);

        // Third row: Identifiers
        let identifiers_block = Block::default()
            .borders(Borders::ALL)
            .title("Identifiers")
            .title_alignment(Alignment::Center);

        // Dynamically generate ASCII-styled big numbers for first and second rows
        let epoch_ascii = number_to_big(self.current_epoch as u128);
        let locked_ascii = number_to_big(self.stake.locked.parse()?);
        let reputation_ascii = number_to_big(self.reputation.parse()?);
        //let uptime_ascii = number_to_big(self.up)
        //let staked_ascii = number_to_big(self.stake.staked.parse()?);
        let staked_ascii = number_to_big(self.stake.staked.parse()?);
        let uptime_ascii = number_to_big(self.uptime.parse()?);

        // Render the first row (epoch, locked, reputation) in titled boxes
        let epoch_block = Block::default()
            .borders(Borders::ALL)
            .title("Epoch")
            .title_alignment(Alignment::Center);
        let locked_block = Block::default()
            .borders(Borders::ALL)
            .title("Locked")
            .title_alignment(Alignment::Center);
        let reputation_block = Block::default()
            .borders(Borders::ALL)
            .title("Reputation")
            .title_alignment(Alignment::Center);
        let uptime_block = Block::default()
        .borders(Borders::ALL)
        .title("Uptime")
        .title_alignment(Alignment::Center);

        f.render_widget(
            Paragraph::new(epoch_ascii).block(epoch_block).alignment(Alignment::Center),
            row1[0],
        );
        f.render_widget(
            Paragraph::new(locked_ascii).block(locked_block).alignment(Alignment::Center),
            row1[1],
        );
        f.render_widget(
            Paragraph::new(reputation_ascii).block(reputation_block).alignment(Alignment::Center),
            row1[2],
        );
        f.render_widget(
            Paragraph::new(uptime_ascii).block(uptime_block).alignment(Alignment::Center),
            row1[3],
        );

        // Render the second row (staked) in a titled box
        let staked_block = Block::default()
            .borders(Borders::ALL)
            .title("Staked")
            .title_alignment(Alignment::Center);

        f.render_widget(
            Paragraph::new(staked_ascii).block(staked_block).alignment(Alignment::Center),
            row2[0],
        );

        // Render the third row (Identifiers)
        let identifiers_content = vec![
            format!("Ethereum Address: {}", self.ethereum_address),
            format!("Public Key: {}", self.public_key),
            format!("Consensus Key: {}", self.consensus_key),
            format!("Participation: {}", self.participation),
            format!("Stake Locked Until: {}", self.stake.stake_locked_until),
            format!("Locked Until: {}", self.stake.locked_until),
            format!("The rest of the fields is for debugging purposes, place them in boxes later"),
            format!("Staked: {}", self.stake.staked),
            format!("Committee Members: {}", self.committee_members.join(", ")),
        ]
            .join("\n");

        let identifiers_paragraph = Paragraph::new(identifiers_content)
            .block(identifiers_block)
            .alignment(Alignment::Left);

        f.render_widget(identifiers_paragraph, vchunks[2]);

        Ok(())
    }


    // fn draw(&mut self, f: &mut Frame<'_>, area: Rect) -> Result<()> {
    //     // Split the main area into sections
    //     let vchunks = Layout::vertical([
    //         Constraint::Length(7),  // First row for big numbers
    //         Constraint::Min(10),    // Remaining fields
    //         Constraint::Length(5),  // Identifiers row
    //     ])
    //         .split(area);
    //
    //     // Horizontal layout for the first row
    //     let row1 = Layout::horizontal([
    //         Constraint::Percentage(20), // Epoch
    //         Constraint::Percentage(20), // Locked
    //         Constraint::Percentage(20), // Reputation
    //         Constraint::Percentage(20), // Locked Until
    //         Constraint::Percentage(20), // Stake Locked Until
    //     ])
    //         .split(vchunks[0]);
    //
    //     // Dynamically generate ASCII-styled big numbers
    //     let epoch_ascii = number_to_big(self.current_epoch);
    //     let locked_ascii = number_to_big(self.stake.locked.parse()?);
    //     let reputation_ascii = number_to_big(self.reputation.parse()?);
    //     let locked_until_ascii = number_to_big(self.stake.locked_until);
    //     let stake_locked_until_ascii = number_to_big(self.stake.stake_locked_until);
    //
    //     // Render each big number in its respective block
    //     f.render_widget(
    //         Paragraph::new(epoch_ascii).alignment(Alignment::Center),
    //         row1[0],
    //     );
    //     f.render_widget(
    //         Paragraph::new(locked_ascii).alignment(Alignment::Center),
    //         row1[1],
    //     );
    //     f.render_widget(
    //         Paragraph::new(reputation_ascii).alignment(Alignment::Center),
    //         row1[2],
    //     );
    //     f.render_widget(
    //         Paragraph::new(locked_until_ascii).alignment(Alignment::Center),
    //         row1[3],
    //     );
    //     f.render_widget(
    //         Paragraph::new(stake_locked_until_ascii).alignment(Alignment::Center),
    //         row1[4],
    //     );
    //
    //     Ok(())
    // }

    //TODO: Update draw
    // fn draw1(&mut self, f: &mut Frame<'_>, area: Rect) -> Result<()> {
    //     let vchunks = Layout::vertical([
    //         Constraint::Percentage(30),
    //         Constraint::Percentage(40),
    //         Constraint::Percentage(30),
    //     ])
    //     .split(area);
    //
    //     let hchunks = Layout::horizontal([
    //         Constraint::Fill(1),
    //         Constraint::Max(500),
    //         Constraint::Fill(1),
    //     ])
    //     .split(vchunks[1]);
    //
    //     let logo = Layout::horizontal([Constraint::Length(12), Constraint::Fill(1)]).split(hchunks[1]);
    //     let title = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).split(logo[1]);
    //     let metrics = List::new(vec![
    //         ListItem::new(
    //             Text::from(Line::from(vec![
    //                 Span::raw("This is the current epoch: "),
    //                 Span::styled(
    //                     format!("{}", self.current_epoch),
    //                     Style::default().fg(Color::Cyan),
    //                 ),
    //             ])),
    //         ),
    //         ListItem::new(
    //             Text::from(Line::from(vec![
    //                 Span::raw("This is the current Ethereum address: "),
    //                 Span::styled(
    //                     format!("{}", self.ethereum_address),
    //                     Style::default().fg(Color::Cyan),
    //                 ),
    //             ])),
    //         ),
    //         ListItem::new(
    //             Text::from(Line::from(vec![
    //                 Span::raw("This is the current public key: "),
    //                 Span::styled(
    //                     format!("{}", self.public_key),
    //                     Style::default().fg(Color::Cyan),
    //                 ),
    //             ])),
    //         ),
    //         ListItem::new(
    //             Text::from(Line::from(vec![
    //                 Span::raw("This is the current consensus key: "),
    //                 Span::styled(
    //                     format!("{}", self.consensus_key),
    //                     Style::default().fg(Color::Cyan),
    //                 ),
    //             ])),
    //         ),
    //         ListItem::new(
    //             Text::from(Line::from(vec![
    //                 Span::raw("This is the current participation status: "),
    //                 Span::styled(
    //                     format!("{}", self.participation),
    //                     Style::default().fg(Color::Cyan),
    //                 ),
    //             ])),
    //         ),
    //         ListItem::new(
    //             Text::from(Line::from(vec![
    //                 Span::raw("Staked: "),
    //                 Span::styled(
    //                     format!("{}", self.stake.staked),
    //                     Style::default().fg(Color::Cyan),
    //                 ),
    //             ])),
    //         ),
    //         ListItem::new(
    //             Text::from(Line::from(vec![
    //                 Span::raw("Stake locked until: "),
    //                 Span::styled(
    //                     format!("{}", self.stake.stake_locked_until),
    //                     Style::default().fg(Color::Cyan),
    //                 ),
    //             ])),
    //         ),
    //         ListItem::new(
    //             Text::from(Line::from(vec![
    //                 Span::raw("Locked: "),
    //                 Span::styled(
    //                     format!("{}", self.stake.locked),
    //                     Style::default().fg(Color::Cyan),
    //                 ),
    //             ])),
    //         ),
    //         ListItem::new(
    //             Text::from(Line::from(vec![
    //                 Span::raw("Locked until: "),
    //                 Span::styled(
    //                     format!("{}", self.stake.locked_until),
    //                     Style::default().fg(Color::Cyan),
    //                 ),
    //             ])),
    //         ),
    //         ListItem::new(
    //             Text::from(Line::from(vec![
    //                 Span::raw("Work bitch you "),
    //             ]))
    //         ),
    //         ListItem::new(
    //             Text::from(Line::from(vec![
    //                 Span::raw("Participation: "),
    //                 Span::styled(
    //                     format!("{}", self.participation),
    //                     Style::default().fg(Color::Cyan),
    //                 )
    //             ]))
    //         ),
    //         ListItem::new(
    //             Text::from(Line::from(vec![
    //                 Span::raw("Reputation: "),
    //                 Span::styled(
    //                     format!("{}", self.reputation),
    //                     Style::default().fg(Color::Cyan),
    //                 )
    //             ]))
    //         ),
    //     ])
    //         .block(
    //             Block::default()
    //             .borders(Borders::ALL)
    //             .border_style(Style::default().fg(Color::White))
    //                 .title_alignment(Alignment::Center)
    //                 .title("Metrics")
    //         );
    //     //f.render_widget(Paragraph::new(format!("This is the current epoch:{}\nThis is the current ethereum address:{}",self.current_epoch,self.ethereum_address)).alignment(Alignment::Left), title[0]);
    //     f.render_widget(metrics, title[0]);
    //     Ok(())
    // }

}
