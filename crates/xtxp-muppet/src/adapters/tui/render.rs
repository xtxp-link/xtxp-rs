//! Drawing: header, tree pane, detail pane, status line.

use super::model::{display_name, find, rows, EditTarget};
use super::model::{InputMode, Model};
use crate::ports::Screen;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use xmesh::IdNodeKind;

const HINTS: &str = "↑↓ select  ←→ fold  Enter edit  a add  d delete  i insert  m move  q quit";

/// Draws one frame: header, tree pane, detail pane, status line.
pub fn draw(frame: &mut Frame, model: &Model, screen: &Screen, now_ms: u64) {
    let [header, body, status] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .areas(frame.area());
    let [tree_area, detail_area] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .areas(body);

    let connection = if screen.connected {
        "● live"
    } else {
        "○ reconnecting"
    };
    let doc_hash: String = screen.doc_hash.chars().take(12).collect();
    frame.render_widget(
        Paragraph::new(format!(
            " {}  @{}  doc {}  {}",
            model.doc, screen.snapshot_hash, doc_hash, connection
        ))
        .style(Style::default().add_modifier(Modifier::BOLD)),
        header,
    );

    let visible = rows(model, &screen.tree);
    let items: Vec<ListItem> = visible
        .iter()
        .map(|r| {
            let fold = match (r.has_children, model.folded.contains(&r.id)) {
                (true, true) => "▸ ",
                (true, false) => "▾ ",
                _ => "  ",
            };
            let mut style = Style::default();
            if model
                .highlight_until
                .get(&r.id)
                .is_some_and(|until| *until > now_ms)
            {
                style = style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            }
            if matches!(model.mode, InputMode::MovePending(n) if n == r.id) {
                style = style.add_modifier(Modifier::ITALIC);
            }
            ListItem::new(format!("{}{}{}", "  ".repeat(r.depth), fold, r.label)).style(style)
        })
        .collect();
    let mut state = ListState::default();
    state.select(visible.iter().position(|r| Some(r.id) == model.selected));
    frame.render_stateful_widget(
        List::new(items)
            .block(Block::default().borders(Borders::ALL).title(" tree "))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        tree_area,
        &mut state,
    );

    let detail: Vec<Line> = match model
        .selected
        .and_then(|id| find(&screen.tree, id))
        .map(|n| &n.node)
    {
        Some(IdNodeKind::Element { attrs, .. }) if attrs.is_empty() => {
            vec![Line::from("(no attributes)")]
        }
        Some(IdNodeKind::Element { attrs, .. }) => attrs
            .iter()
            .enumerate()
            .map(|(i, (k, v))| {
                let line = Line::from(format!("{} = {}", display_name(k), v));
                if matches!(model.mode, InputMode::AttrList { index } if index == i) {
                    line.style(Style::default().add_modifier(Modifier::REVERSED))
                } else {
                    line
                }
            })
            .collect(),
        Some(IdNodeKind::Text(t)) => t.lines().map(|l| Line::from(l.to_string())).collect(),
        None => vec![],
    };
    frame.render_widget(
        Paragraph::new(detail)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(" detail ")),
        detail_area,
    );

    let line = match &model.mode {
        InputMode::EditValue {
            target: EditTarget::Attr(_, name),
            buffer,
        } => format!("{} = {buffer}▏", display_name(name)),
        InputMode::EditValue {
            target: EditTarget::Text(_),
            buffer,
        } => format!("text: {buffer}▏"),
        InputMode::AddAttrName { buffer } => format!("new attribute name: {buffer}▏"),
        InputMode::AddAttrValue { name, buffer } => format!("{name} = {buffer}▏"),
        InputMode::InsertXml { buffer } => format!("insert first child (XML): {buffer}▏"),
        InputMode::MovePending(_) => "move: select the new parent and press m; Esc cancels".into(),
        InputMode::AttrList { .. } => "Enter edit  a add  x remove  Esc back".into(),
        InputMode::Browse if !model.status.is_empty() => model.status.clone(),
        InputMode::Browse => HINTS.into(),
    };
    frame.render_widget(Paragraph::new(line), status);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use xmesh::Xmesh;

    #[test]
    fn draws_header_tree_detail_status() {
        let d = Xmesh::new(1).unwrap();
        d.apply(
            &Xmesh::seed(
                r#"<svg xmlns="http://www.w3.org/2000/svg"><rect id="r1" fill="red"/></svg>"#,
            )
            .unwrap(),
        )
        .unwrap();
        let screen = Screen {
            tree: d.tree_with_ids(),
            snapshot_hash: d.snapshot_hash(),
            doc_hash: d.hash(),
            connected: true,
            changed: vec![],
        };
        let mut model = super::super::model::Model::new("demo", false);
        super::super::keys::after_show(&mut model, &Screen::default(), &screen, 0);
        model.status = "hello status".into();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw(f, &model, &screen, 0)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("demo"), "{text}");
        assert!(
            text.contains(&screen.snapshot_hash),
            "snapshot hash in the header"
        );
        assert!(text.contains("● live"));
        assert!(text.contains("<rect id=\"r1\">"));
        assert!(text.contains("hello status"));
    }

    #[test]
    fn detail_shows_attributes_of_selection() {
        let d = Xmesh::new(1).unwrap();
        d.apply(
            &Xmesh::seed(
                r#"<svg xmlns="http://www.w3.org/2000/svg"><rect id="r1" fill="red"/></svg>"#,
            )
            .unwrap(),
        )
        .unwrap();
        let screen = Screen {
            tree: d.tree_with_ids(),
            snapshot_hash: d.snapshot_hash(),
            doc_hash: d.hash(),
            connected: false,
            changed: vec![],
        };
        let mut model = super::super::model::Model::new("demo", false);
        super::super::keys::after_show(&mut model, &Screen::default(), &screen, 0);
        let rect = match &super::super::model::root(&screen.tree).unwrap().node {
            xmesh::IdNodeKind::Element { children, .. } => children[0].id,
            _ => unreachable!(),
        };
        model.selected = Some(rect);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw(f, &model, &screen, 0)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("fill = red"), "{text}");
        assert!(text.contains("○ reconnecting"));
    }
}
