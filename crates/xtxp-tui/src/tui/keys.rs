//! Keys to commands. Pure: takes the model and screen, returns commands.

use super::model::{find, root, rows};
use super::model::{EditTarget, InputMode, Model};
use xtxp_muppet::core::Command;
use xtxp_muppet::ports::Screen;
use ratatui::crossterm::event::KeyEventKind;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::BTreeMap;
use xmesh::{IdNode, IdNodeKind, NodeId};

const HIGHLIGHT_MS: u64 = 1_000;

/// Updates the model for a new screen: keeps the selection on a live node
/// (its nearest live ancestor if it was deleted), records parent links, and
/// highlights changed nodes for a second.
pub fn after_show(model: &mut Model, old: &Screen, new: &Screen, now_ms: u64) {
    let _ = old;
    match model.selected {
        None => model.selected = root(&new.tree).map(|r| r.id),
        Some(sel) if find(&new.tree, sel).is_none() => {
            let mut up = model.parents.get(&sel).copied();
            while let Some(p) = up {
                if find(&new.tree, p).is_some() {
                    break;
                }
                up = model.parents.get(&p).copied();
            }
            model.selected = up.or_else(|| root(&new.tree).map(|r| r.id));
            model.status =
                "the selected node was deleted by someone else; selection moved to its parent"
                    .into();
            if !matches!(model.mode, InputMode::Browse) {
                model.mode = InputMode::Browse;
            }
        }
        Some(_) => {}
    }
    model.parents.clear();
    fn link(nodes: &[IdNode], parent: Option<NodeId>, out: &mut BTreeMap<NodeId, NodeId>) {
        for n in nodes {
            if let Some(p) = parent {
                out.insert(n.id, p);
            }
            if let IdNodeKind::Element { children, .. } = &n.node {
                link(children, Some(n.id), out);
            }
        }
    }
    link(&new.tree, None, &mut model.parents);
    model.highlight_until.retain(|_, until| *until > now_ms);
    for id in &new.changed {
        model.highlight_until.insert(*id, now_ms + HIGHLIGHT_MS);
    }
}

fn selected_node<'a>(model: &Model, screen: &'a Screen) -> Option<&'a IdNode> {
    model.selected.and_then(|id| find(&screen.tree, id))
}

fn attrs_of(node: &IdNode) -> Vec<(String, String)> {
    match &node.node {
        IdNodeKind::Element { attrs, .. } => attrs.clone(),
        IdNodeKind::Text(_) => vec![],
    }
}

fn seen(screen: &Screen) -> Command {
    Command::Seen {
        snapshot: screen.snapshot_hash.clone(),
    }
}

/// Handles one key press; returns the commands it produces.
pub fn handle(model: &mut Model, screen: &Screen, key: KeyEvent) -> Vec<Command> {
    if key.kind != KeyEventKind::Press {
        return vec![];
    }
    // ^C quits from anywhere, as in other terminal apps; a value still being
    // typed is dropped, and everything already applied has been sent.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        model.mode = InputMode::Browse;
        model.quit = true;
        return vec![];
    }
    let mode = model.mode.clone();
    match mode {
        InputMode::Browse | InputMode::MovePending(_) => browse(model, screen, key),
        InputMode::AttrList { index } => attr_list(model, screen, key, index),
        InputMode::EditValue { target, mut buffer } => match key.code {
            KeyCode::Enter => {
                model.mode = InputMode::Browse;
                let command = match target {
                    EditTarget::Attr(node, name) => Command::SetAttr {
                        node,
                        name,
                        value: buffer,
                    },
                    EditTarget::Text(node) => Command::SetText { node, text: buffer },
                };
                vec![seen(screen), command]
            }
            KeyCode::Esc => {
                model.mode = InputMode::Browse;
                vec![]
            }
            other => {
                type_into(&mut buffer, other);
                model.mode = InputMode::EditValue { target, buffer };
                vec![]
            }
        },
        InputMode::AddAttrName { mut buffer } => match key.code {
            KeyCode::Enter if !buffer.is_empty() => {
                model.mode = InputMode::AddAttrValue {
                    name: buffer,
                    buffer: String::new(),
                };
                vec![]
            }
            KeyCode::Esc => {
                model.mode = InputMode::Browse;
                vec![]
            }
            other => {
                type_into(&mut buffer, other);
                model.mode = InputMode::AddAttrName { buffer };
                vec![]
            }
        },
        InputMode::AddAttrValue { name, mut buffer } => match key.code {
            KeyCode::Enter => {
                model.mode = InputMode::Browse;
                match model.selected {
                    Some(node) => vec![
                        seen(screen),
                        Command::SetAttr {
                            node,
                            name,
                            value: buffer,
                        },
                    ],
                    None => vec![],
                }
            }
            KeyCode::Esc => {
                model.mode = InputMode::Browse;
                vec![]
            }
            other => {
                type_into(&mut buffer, other);
                model.mode = InputMode::AddAttrValue { name, buffer };
                vec![]
            }
        },
        InputMode::InsertXml { mut buffer } => match key.code {
            KeyCode::Enter if !buffer.is_empty() => {
                model.mode = InputMode::Browse;
                match model.selected {
                    Some(parent) => vec![
                        seen(screen),
                        Command::InsertChild {
                            parent,
                            xml: buffer,
                        },
                    ],
                    None => vec![],
                }
            }
            KeyCode::Esc => {
                model.mode = InputMode::Browse;
                vec![]
            }
            other => {
                type_into(&mut buffer, other);
                model.mode = InputMode::InsertXml { buffer };
                vec![]
            }
        },
    }
}

fn type_into(buffer: &mut String, code: KeyCode) {
    match code {
        KeyCode::Char(c) => buffer.push(c),
        KeyCode::Backspace => {
            buffer.pop();
        }
        _ => {}
    }
}

fn editing_allowed(model: &mut Model) -> bool {
    if model.read_only {
        model.status = "read-only: editing keys are off".into();
    }
    !model.read_only
}

fn browse(model: &mut Model, screen: &Screen, key: KeyEvent) -> Vec<Command> {
    let visible = rows(model, &screen.tree);
    let at = visible
        .iter()
        .position(|r| Some(r.id) == model.selected)
        .unwrap_or(0);
    let node = selected_node(model, screen).cloned();
    let is_element = matches!(
        node.as_ref().map(|n| &n.node),
        Some(IdNodeKind::Element { .. })
    );
    let is_root = node.as_ref().map(|n| n.id) == root(&screen.tree).map(|r| r.id);
    match key.code {
        KeyCode::Up if at > 0 => model.selected = Some(visible[at - 1].id),
        KeyCode::Down if at + 1 < visible.len() => model.selected = Some(visible[at + 1].id),
        KeyCode::Left => {
            if let Some(sel) = model.selected {
                if visible.get(at).is_some_and(|r| r.has_children) && !model.folded.contains(&sel) {
                    model.folded.insert(sel);
                } else if let Some(parent) = model.parents.get(&sel) {
                    model.selected = Some(*parent);
                }
            }
        }
        KeyCode::Right => {
            if let Some(sel) = model.selected {
                model.folded.remove(&sel);
            }
        }
        KeyCode::Char('q') => model.quit = true,
        KeyCode::Esc => model.mode = InputMode::Browse,
        KeyCode::Enter => match node.map(|n| n.node) {
            Some(IdNodeKind::Element { .. }) => model.mode = InputMode::AttrList { index: 0 },
            Some(IdNodeKind::Text(text)) if editing_allowed(model) => {
                let target = EditTarget::Text(model.selected.expect("a node is selected"));
                model.mode = InputMode::EditValue {
                    target,
                    buffer: text,
                };
            }
            _ => {}
        },
        KeyCode::Char('a') if is_element && editing_allowed(model) => {
            model.mode = InputMode::AddAttrName {
                buffer: String::new(),
            }
        }
        KeyCode::Char('i') if is_element && editing_allowed(model) => {
            model.mode = InputMode::InsertXml {
                buffer: String::new(),
            }
        }
        KeyCode::Char('d') if editing_allowed(model) => {
            if is_root {
                model.status = "the root element can't be deleted".into();
            } else if let Some(node) = model.selected {
                return vec![seen(screen), Command::Delete { node }];
            }
        }
        KeyCode::Char('m') if editing_allowed(model) => {
            match (model.mode.clone(), model.selected) {
                (InputMode::MovePending(node), Some(target)) if is_element && target != node => {
                    model.mode = InputMode::Browse;
                    return vec![
                        seen(screen),
                        Command::Move {
                            node,
                            new_parent: target,
                        },
                    ];
                }
                (InputMode::Browse, Some(node)) if !is_root => {
                    model.mode = InputMode::MovePending(node);
                }
                _ => {}
            }
        }
        _ => {}
    }
    vec![]
}

fn attr_list(model: &mut Model, screen: &Screen, key: KeyEvent, index: usize) -> Vec<Command> {
    let Some(node) = selected_node(model, screen).cloned() else {
        model.mode = InputMode::Browse;
        return vec![];
    };
    let attrs = attrs_of(&node);
    // Index `attrs.len()` is the "+ add attribute" row. Someone else may have
    // removed attributes since the index was set, so clamp it.
    let index = index.min(attrs.len());
    let add_name = || InputMode::AddAttrName {
        buffer: String::new(),
    };
    match key.code {
        KeyCode::Up if index > 0 => model.mode = InputMode::AttrList { index: index - 1 },
        KeyCode::Down if index < attrs.len() => {
            model.mode = InputMode::AttrList { index: index + 1 }
        }
        KeyCode::Esc => model.mode = InputMode::Browse,
        KeyCode::Enter if editing_allowed(model) => match attrs.get(index) {
            Some((name, value)) => {
                model.mode = InputMode::EditValue {
                    target: EditTarget::Attr(node.id, name.clone()),
                    buffer: value.clone(),
                }
            }
            None => model.mode = add_name(),
        },
        KeyCode::Left | KeyCode::Right if editing_allowed(model) => {
            let size = if key.modifiers.contains(KeyModifiers::SHIFT) {
                10
            } else {
                1
            };
            let steps = if key.code == KeyCode::Right {
                size
            } else {
                -size
            };
            model.mode = InputMode::AttrList { index };
            if let Some((name, value)) = attrs.get(index) {
                if let Some(value) = nudge(value, steps) {
                    return vec![
                        seen(screen),
                        Command::SetAttr {
                            node: node.id,
                            name: name.clone(),
                            value,
                        },
                    ];
                }
            }
        }
        KeyCode::Char('a') if editing_allowed(model) => model.mode = add_name(),
        KeyCode::Char('x') | KeyCode::Backspace | KeyCode::Delete if editing_allowed(model) => {
            if let Some((name, _)) = attrs.get(index) {
                model.mode = InputMode::AttrList {
                    index: index.saturating_sub(1),
                };
                return vec![
                    seen(screen),
                    Command::RemoveAttr {
                        node: node.id,
                        name: name.clone(),
                    },
                ];
            }
        }
        _ => model.mode = InputMode::AttrList { index },
    }
    vec![]
}

/// A value that is one number with an optional unit (`20`, `-1.5`, `50px`,
/// `100%`): its digits as an integer, its number of decimals, and the unit.
fn parse_number(value: &str) -> Option<(i128, usize, &str)> {
    let (negative, rest) = match value.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, value),
    };
    let int_len = rest.bytes().take_while(u8::is_ascii_digit).count();
    if int_len == 0 {
        return None;
    }
    let (frac, unit) = match rest[int_len..].strip_prefix('.') {
        Some(after) => {
            let n = after.bytes().take_while(u8::is_ascii_digit).count();
            if n == 0 {
                return None;
            }
            (&after[..n], &after[n..])
        }
        None => ("", &rest[int_len..]),
    };
    if int_len + frac.len() > 30 || !unit.chars().all(|c| c.is_ascii_alphabetic() || c == '%') {
        return None;
    }
    let digits: i128 = format!("{}{frac}", &rest[..int_len]).parse().ok()?;
    Some((if negative { -digits } else { digits }, frac.len(), unit))
}

/// `value` moved by `steps` units of its last digit, keeping its decimals and
/// unit; `None` if it isn't one number.
pub fn nudge(value: &str, steps: i64) -> Option<String> {
    let (digits, decimals, unit) = parse_number(value)?;
    let moved = digits.checked_add(steps as i128)?;
    let abs = format!("{:0>width$}", moved.unsigned_abs(), width = decimals + 1);
    let (int, frac) = abs.split_at(abs.len() - decimals);
    let sign = if moved < 0 { "-" } else { "" };
    Some(if decimals == 0 {
        format!("{sign}{int}{unit}")
    } else {
        format!("{sign}{int}.{frac}{unit}")
    })
}

/// The size of one nudge for `value`, such as `1` or `0.01`, if it's a number.
pub fn nudge_step(value: &str) -> Option<String> {
    let (_, decimals, _) = parse_number(value)?;
    Some(match decimals {
        0 => "1".into(),
        d => format!("0.{}1", "0".repeat(d - 1)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Command;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState, KeyModifiers};
    use xmesh::{IdNode, IdNodeKind, NodeId, Xmesh};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn screen(xml: &str) -> Screen {
        let d = Xmesh::new(1).unwrap();
        d.apply(&Xmesh::seed(xml).unwrap()).unwrap();
        Screen {
            tree: d.tree_with_ids(),
            snapshot_hash: d.snapshot_hash(),
            doc_hash: d.hash(),
            connected: true,
            changed: vec![],
        }
    }

    const SVG: &str =
        r#"<svg xmlns="http://www.w3.org/2000/svg"><rect id="a" fill="red"/><text>hi</text></svg>"#;

    fn ids(s: &Screen) -> (NodeId, NodeId, NodeId, NodeId) {
        let root = super::super::model::root(&s.tree).unwrap();
        let IdNodeKind::Element { children, .. } = &root.node else {
            unreachable!()
        };
        let IdNodeKind::Element {
            children: text_children,
            ..
        } = &children[1].node
        else {
            unreachable!()
        };
        (root.id, children[0].id, children[1].id, text_children[0].id)
    }

    fn model_on(s: &Screen, node: NodeId) -> Model {
        let mut m = Model::new("demo", false);
        after_show(&mut m, &Screen::default(), s, 0);
        m.selected = Some(node);
        m
    }

    fn press(m: &mut Model, s: &Screen, codes: &[KeyCode]) -> Vec<Command> {
        codes.iter().flat_map(|c| handle(m, s, key(*c))).collect()
    }

    fn typed(text: &str) -> Vec<KeyCode> {
        text.chars().map(KeyCode::Char).collect()
    }

    #[test]
    fn down_and_up_move_through_visible_rows() {
        let s = screen(SVG);
        let (root, rect, text, _) = ids(&s);
        let mut m = model_on(&s, root);
        press(&mut m, &s, &[KeyCode::Down]);
        assert_eq!(m.selected, Some(rect));
        press(&mut m, &s, &[KeyCode::Down]);
        assert_eq!(m.selected, Some(text));
        press(&mut m, &s, &[KeyCode::Up]);
        assert_eq!(m.selected, Some(rect));
    }

    #[test]
    fn enter_on_element_opens_attr_list() {
        let s = screen(SVG);
        let (_, rect, _, _) = ids(&s);
        let mut m = model_on(&s, rect);
        assert!(press(&mut m, &s, &[KeyCode::Enter]).is_empty());
        assert_eq!(m.mode, InputMode::AttrList { index: 0 });
    }

    #[test]
    fn edit_value_then_enter_sends_set_attr() {
        let s = screen(SVG);
        let (_, rect, _, _) = ids(&s);
        let mut m = model_on(&s, rect);
        // Attributes in canonical order: fill, id. Edit fill.
        press(&mut m, &s, &[KeyCode::Enter, KeyCode::Enter]);
        assert_eq!(
            m.mode,
            InputMode::EditValue {
                target: EditTarget::Attr(rect, "fill".into()),
                buffer: "red".into()
            }
        );
        let mut keys = vec![KeyCode::Backspace; 3];
        keys.extend(typed("blue"));
        keys.push(KeyCode::Enter);
        let cmds = press(&mut m, &s, &keys);
        assert_eq!(
            cmds,
            vec![
                Command::Seen {
                    snapshot: s.snapshot_hash.clone()
                },
                Command::SetAttr {
                    node: rect,
                    name: "fill".into(),
                    value: "blue".into()
                }
            ]
        );
        assert_eq!(m.mode, InputMode::Browse);
    }

    #[test]
    fn enter_on_text_edits_it() {
        let s = screen(SVG);
        let (_, _, _, hi) = ids(&s);
        let mut m = model_on(&s, hi);
        let mut keys = vec![KeyCode::Enter];
        keys.extend(typed("!"));
        keys.push(KeyCode::Enter);
        let cmds = press(&mut m, &s, &keys);
        assert_eq!(
            cmds.last(),
            Some(&Command::SetText {
                node: hi,
                text: "hi!".into()
            })
        );
    }

    #[test]
    fn esc_cancels() {
        let s = screen(SVG);
        let (_, rect, _, _) = ids(&s);
        let mut m = model_on(&s, rect);
        let cmds = press(
            &mut m,
            &s,
            &[
                KeyCode::Enter,
                KeyCode::Enter,
                KeyCode::Char('x'),
                KeyCode::Esc,
            ],
        );
        assert!(cmds.is_empty());
        assert_eq!(m.mode, InputMode::Browse);
    }

    #[test]
    fn x_in_attr_list_removes() {
        let s = screen(SVG);
        let (_, rect, _, _) = ids(&s);
        let mut m = model_on(&s, rect);
        let cmds = press(
            &mut m,
            &s,
            &[KeyCode::Enter, KeyCode::Down, KeyCode::Char('x')],
        );
        assert_eq!(
            cmds.last(),
            Some(&Command::RemoveAttr {
                node: rect,
                name: "id".into()
            })
        );
    }

    #[test]
    fn a_adds_an_attribute() {
        let s = screen(SVG);
        let (_, rect, _, _) = ids(&s);
        let mut m = model_on(&s, rect);
        let mut keys = vec![KeyCode::Char('a')];
        keys.extend(typed("stroke"));
        keys.push(KeyCode::Enter);
        keys.extend(typed("black"));
        keys.push(KeyCode::Enter);
        let cmds = press(&mut m, &s, &keys);
        assert_eq!(
            cmds.last(),
            Some(&Command::SetAttr {
                node: rect,
                name: "stroke".into(),
                value: "black".into()
            })
        );
    }

    #[test]
    fn d_sends_delete_but_not_on_the_root() {
        let s = screen(SVG);
        let (root, rect, _, _) = ids(&s);
        let mut m = model_on(&s, rect);
        assert_eq!(
            press(&mut m, &s, &[KeyCode::Char('d')]).last(),
            Some(&Command::Delete { node: rect })
        );
        let mut m = model_on(&s, root);
        assert!(press(&mut m, &s, &[KeyCode::Char('d')]).is_empty());
    }

    #[test]
    fn i_inserts_a_child() {
        let s = screen(SVG);
        let (root, _, _, _) = ids(&s);
        let mut m = model_on(&s, root);
        let mut keys = vec![KeyCode::Char('i')];
        keys.extend(typed("<circle/>"));
        keys.push(KeyCode::Enter);
        let cmds = press(&mut m, &s, &keys);
        assert_eq!(
            cmds.last(),
            Some(&Command::InsertChild {
                parent: root,
                xml: "<circle/>".into()
            })
        );
    }

    #[test]
    fn m_then_m_sends_move() {
        let s = screen(SVG);
        let (_, rect, text, _) = ids(&s);
        let mut m = model_on(&s, text);
        assert!(press(&mut m, &s, &[KeyCode::Char('m')]).is_empty());
        assert_eq!(m.mode, InputMode::MovePending(text));
        m.selected = Some(rect);
        let cmds = press(&mut m, &s, &[KeyCode::Char('m')]);
        assert_eq!(
            cmds.last(),
            Some(&Command::Move {
                node: text,
                new_parent: rect
            })
        );
        assert_eq!(m.mode, InputMode::Browse);
    }

    #[test]
    fn read_only_ignores_edits() {
        let s = screen(SVG);
        let (_, rect, _, _) = ids(&s);
        let mut m = model_on(&s, rect);
        m.read_only = true;
        assert!(press(
            &mut m,
            &s,
            &[
                KeyCode::Char('d'),
                KeyCode::Char('a'),
                KeyCode::Enter,
                KeyCode::Enter
            ]
        )
        .is_empty());
        assert!(m.status.contains("read-only"));
    }

    #[test]
    fn q_quits() {
        let s = screen(SVG);
        let (root, _, _, _) = ids(&s);
        let mut m = model_on(&s, root);
        press(&mut m, &s, &[KeyCode::Char('q')]);
        assert!(m.quit);
    }

    #[test]
    fn selection_moves_to_ancestor_when_deleted() {
        let old = screen(SVG);
        let (root, rect, _, _) = ids(&old);
        let mut m = model_on(&old, rect);
        // The same document with the rect removed.
        let d = Xmesh::new(1).unwrap();
        d.apply(&Xmesh::seed(SVG).unwrap()).unwrap();
        d.edit(&[xmesh::Op::Remove { path: vec![0] }]).unwrap();
        let new = Screen {
            tree: d.tree_with_ids(),
            snapshot_hash: d.snapshot_hash(),
            doc_hash: d.hash(),
            connected: true,
            changed: vec![],
        };
        after_show(&mut m, &old, &new, 0);
        assert_eq!(m.selected, Some(root));
        assert!(m.status.contains("deleted"), "{}", m.status);
    }

    #[test]
    fn changed_nodes_are_highlighted_for_a_second() {
        let s = screen(SVG);
        let (_, rect, _, _) = ids(&s);
        let mut m = model_on(&s, rect);
        let mut changed = s.clone();
        changed.changed = vec![rect];
        after_show(&mut m, &s, &changed, 5_000);
        assert_eq!(m.highlight_until.get(&rect), Some(&6_000));
        let _: Vec<IdNode> = vec![];
    }
    #[test]
    fn down_past_the_last_attribute_selects_the_add_row() {
        let s = screen(SVG);
        let (_, rect, _, _) = ids(&s);
        let mut m = model_on(&s, rect);
        // fill, id, then the add row at index 2; no further.
        press(
            &mut m,
            &s,
            &[KeyCode::Enter, KeyCode::Down, KeyCode::Down, KeyCode::Down],
        );
        assert_eq!(m.mode, InputMode::AttrList { index: 2 });
        press(&mut m, &s, &[KeyCode::Up]);
        assert_eq!(m.mode, InputMode::AttrList { index: 1 });
    }

    #[test]
    fn enter_on_the_add_row_adds_an_attribute() {
        let s = screen(SVG);
        let (_, rect, _, _) = ids(&s);
        let mut m = model_on(&s, rect);
        press(
            &mut m,
            &s,
            &[KeyCode::Enter, KeyCode::Down, KeyCode::Down, KeyCode::Enter],
        );
        assert_eq!(
            m.mode,
            InputMode::AddAttrName {
                buffer: String::new()
            }
        );
        let mut keys = typed("stroke");
        keys.push(KeyCode::Enter);
        keys.extend(typed("blue"));
        keys.push(KeyCode::Enter);
        let cmds = press(&mut m, &s, &keys);
        assert_eq!(
            cmds.last(),
            Some(&Command::SetAttr {
                node: rect,
                name: "stroke".into(),
                value: "blue".into()
            })
        );
    }

    #[test]
    fn backspace_and_delete_remove_the_selected_attribute() {
        let s = screen(SVG);
        let (_, rect, _, _) = ids(&s);
        for k in [KeyCode::Backspace, KeyCode::Delete] {
            let mut m = model_on(&s, rect);
            let cmds = press(&mut m, &s, &[KeyCode::Enter, KeyCode::Down, k]);
            assert_eq!(
                cmds.last(),
                Some(&Command::RemoveAttr {
                    node: rect,
                    name: "id".into()
                }),
                "{k:?}"
            );
        }
        let mut m = model_on(&s, rect);
        let cmds = press(
            &mut m,
            &s,
            &[
                KeyCode::Enter,
                KeyCode::Down,
                KeyCode::Down,
                KeyCode::Backspace,
            ],
        );
        assert!(
            cmds.is_empty(),
            "nothing to remove on the add row: {cmds:?}"
        );
    }

    #[test]
    fn element_without_attributes_opens_on_the_add_row() {
        let s = screen(SVG);
        let (_, _, text, _) = ids(&s);
        let mut m = model_on(&s, text);
        press(&mut m, &s, &[KeyCode::Enter, KeyCode::Enter]);
        assert_eq!(
            m.mode,
            InputMode::AddAttrName {
                buffer: String::new()
            }
        );
    }

    #[test]
    fn attr_index_is_clamped_after_a_remote_removal() {
        let s = screen(SVG);
        let (_, rect, _, _) = ids(&s);
        let mut m = model_on(&s, rect);
        // As if the list had more attributes before someone removed them.
        m.mode = InputMode::AttrList { index: 7 };
        press(&mut m, &s, &[KeyCode::Up]);
        assert_eq!(m.mode, InputMode::AttrList { index: 1 });
    }
    fn key_with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            modifiers,
            ..key(code)
        }
    }

    #[test]
    fn nudge_steps_the_last_digit_and_keeps_units() {
        assert_eq!(nudge("20", 1).as_deref(), Some("21"));
        assert_eq!(nudge("20", -1).as_deref(), Some("19"));
        assert_eq!(nudge("50px", 1).as_deref(), Some("51px"));
        assert_eq!(nudge("1.5", 1).as_deref(), Some("1.6"));
        assert_eq!(nudge("0.264583", 1).as_deref(), Some("0.264584"));
        assert_eq!(nudge("0.25", -10).as_deref(), Some("0.15"));
        assert_eq!(nudge("0", -1).as_deref(), Some("-1"));
        assert_eq!(nudge("-0.1", 1).as_deref(), Some("0.0"));
        assert_eq!(nudge("-0.1", -1).as_deref(), Some("-0.2"));
        assert_eq!(nudge("100%", 10).as_deref(), Some("110%"));
        for not_a_number in [
            "",
            "a",
            "style",
            "0 0 210 297",
            "1.",
            ".5",
            "fill:#ff0000",
            "1e3",
            "-",
        ] {
            assert_eq!(nudge(not_a_number, 1), None, "{not_a_number:?}");
        }
    }

    const NUMS: &str = r#"<svg xmlns="http://www.w3.org/2000/svg"><rect opacity="0.25" width="50px" x="20" id="r"/></svg>"#;

    fn rect_of(s: &Screen) -> NodeId {
        let root = super::super::model::root(&s.tree).unwrap();
        let IdNodeKind::Element { children, .. } = &root.node else {
            unreachable!()
        };
        children[0].id
    }

    #[test]
    fn arrows_nudge_a_numeric_attribute() {
        let s = screen(NUMS);
        let rect = rect_of(&s);
        // Canonical order: id, opacity, width, x.
        let mut m = model_on(&s, rect);
        press(
            &mut m,
            &s,
            &[KeyCode::Enter, KeyCode::Down, KeyCode::Down, KeyCode::Down],
        );
        let cmds = press(&mut m, &s, &[KeyCode::Right]);
        assert_eq!(
            cmds.last(),
            Some(&Command::SetAttr {
                node: rect,
                name: "x".into(),
                value: "21".into()
            })
        );
        assert_eq!(
            m.mode,
            InputMode::AttrList { index: 3 },
            "stays in the list"
        );
        let cmds: Vec<Command> = handle(&mut m, &s, key_with(KeyCode::Left, KeyModifiers::SHIFT));
        assert_eq!(
            cmds.last(),
            Some(&Command::SetAttr {
                node: rect,
                name: "x".into(),
                value: "10".into()
            })
        );
        // On `id` the arrows do nothing.
        let mut m = model_on(&s, rect);
        let cmds = press(&mut m, &s, &[KeyCode::Enter, KeyCode::Right, KeyCode::Left]);
        assert!(cmds.is_empty(), "{cmds:?}");
    }

    #[test]
    fn ctrl_c_quits_from_anywhere_and_drops_a_half_typed_value() {
        let s = screen(SVG);
        let (_, rect, _, _) = ids(&s);
        let ctrl_c = key_with(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let mut m = model_on(&s, rect);
        assert!(handle(&mut m, &s, ctrl_c).is_empty());
        assert!(m.quit, "from browse");
        let mut m = model_on(&s, rect);
        press(&mut m, &s, &[KeyCode::Enter]);
        assert!(handle(&mut m, &s, ctrl_c).is_empty());
        assert!(m.quit, "from the attribute list");
        let mut m = model_on(&s, rect);
        let mut keys = vec![KeyCode::Enter, KeyCode::Enter];
        keys.extend(typed("blu"));
        press(&mut m, &s, &keys);
        assert!(handle(&mut m, &s, ctrl_c).is_empty(), "nothing is sent");
        assert!(m.quit, "mid-typing");
        assert_eq!(m.mode, InputMode::Browse);
    }
}
