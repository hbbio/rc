use std::collections::HashMap;

use crate::keymap::KeyContext;

const HELP_INDEX_ID: &str = "index";
const HELP_PAGE_STEP: usize = 10;
const HELP_HALF_PAGE_STEP: usize = 5;

const HELP_NODE_SPECS: &[(&str, &str, &str)] = &[
    (
        HELP_INDEX_ID,
        "Help",
        "Welcome to rc help.\n\
\n\
Choose a topic:\n\
  [General movement keys](help-viewer)\n\
  [File manager](file-manager)\n\
  [Viewer](viewer)\n\
  [Jobs screen](jobs)\n\
  [Find results](find-results)\n\
  [Directory tree](tree)\n\
  [Directory hotlist](hotlist)\n\
\n\
Use TAB / Shift-TAB to move across links and ENTER to follow.",
    ),
    (
        "help-viewer",
        "Help Viewer",
        "The help viewer supports linked nodes and history.\n\
\n\
Main keys:\n\
  TAB / Shift-TAB  select next/previous link\n\
  ENTER / Right    follow selected link\n\
  Left / F3 / l    go back in history\n\
  F2 / c           open index\n\
  n / p            next / previous node\n\
  F10 / Esc        close help\n\
\n\
Related topics: [File manager](file-manager), [Viewer](viewer), [Jobs](jobs).",
    ),
    (
        "file-manager",
        "File Manager",
        "File manager quick keys:\n\
  Tab switch panel\n\
  Enter/F3 open directory or view file\n\
  Backspace go to parent directory\n\
  Alt-F open find/back to find results\n\
  Alt-T open directory tree\n\
  Alt-H open directory hotlist\n\
  Ctrl-X ! (or Alt/Ctrl-P) open external panelize\n\
  Ctrl-J open jobs screen\n\
  Alt-J cancel latest job\n\
  Alt-S/Ctrl-K open skin picker\n\
  q/F10 quit\n\
\n\
File operations:\n\
  Up/Down move cursor\n\
  Insert/Ctrl-T toggle selection\n\
  F5/F6/F8 copy/move/delete\n\
\n\
More: [Find results](find-results), [Directory tree](tree), [Directory hotlist](hotlist).",
    ),
    (
        "viewer",
        "Viewer",
        "Viewer basics:\n\
  Up/Down and PgUp/PgDn scroll\n\
  F7 search, Shift-F7 reverse search\n\
  n / Shift-n continue search\n\
  g goto line or offset\n\
  w toggle wrap\n\
  h toggle hex mode\n\
\n\
Return to [File manager](file-manager).",
    ),
    (
        "jobs",
        "Jobs",
        "Jobs screen shows queued/running/finished jobs.\n\
\n\
Keys:\n\
  Up/Down move across jobs\n\
  Alt-J cancel selected job\n\
  Esc/q close jobs screen\n\
\n\
Back to [File manager](file-manager).",
    ),
    (
        "find-results",
        "Find Results",
        "Find results are streamed while the search runs.\n\
\n\
Keys:\n\
  Up/Down move\n\
  PgUp/PgDn/Home/End navigate\n\
  Enter locate the result in panel\n\
  Alt-J cancel active find job\n\
  Esc/q close\n\
\n\
See also [File manager](file-manager).",
    ),
    (
        "tree",
        "Directory Tree",
        "Tree screen presents a compact directory tree.\n\
\n\
Keys:\n\
  Up/Down move\n\
  PgUp/PgDn/Home/End navigate\n\
  Enter open selected directory in active panel\n\
  Esc/q close\n\
\n\
See also [Directory hotlist](hotlist) and [File manager](file-manager).",
    ),
    (
        "hotlist",
        "Directory Hotlist",
        "Hotlist stores frequently used directories.\n\
\n\
Keys:\n\
  Enter open selected directory\n\
  a add current directory\n\
  d/delete remove selected entry\n\
  Esc/q close\n\
\n\
See also [Directory tree](tree) and [File manager](file-manager).",
    ),
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HelpSpan {
    Text(String),
    Link { label: String, link_index: usize },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HelpLine {
    pub spans: Vec<HelpSpan>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HelpLink {
    target: String,
    line: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HelpNode {
    id: String,
    title: String,
    lines: Vec<HelpLine>,
    links: Vec<HelpLink>,
}

impl HelpNode {
    fn link_target(&self, index: usize) -> Option<&str> {
        self.links.get(index).map(|link| link.target.as_str())
    }

    fn link_line(&self, index: usize) -> Option<usize> {
        self.links.get(index).map(|link| link.line)
    }
}

#[derive(Clone, Debug)]
pub struct HelpState {
    nodes: Vec<HelpNode>,
    index_by_id: HashMap<String, usize>,
    current_node: usize,
    scroll: usize,
    selected_link: Option<usize>,
    history: Vec<usize>,
}

impl HelpState {
    pub fn for_context(context: KeyContext) -> Self {
        let (nodes, index_by_id) = build_nodes();
        let mut state = Self {
            nodes,
            index_by_id,
            current_node: 0,
            scroll: 0,
            selected_link: None,
            history: Vec::new(),
        };
        state.open_topic(topic_for_context(context), false);
        state
    }

    pub fn open_for_context(&mut self, context: KeyContext) {
        self.open_topic(topic_for_context(context), true);
    }

    pub fn current_title(&self) -> &str {
        self.current_node().title.as_str()
    }

    pub fn current_id(&self) -> &str {
        self.current_node().id.as_str()
    }

    pub fn lines(&self) -> &[HelpLine] {
        self.current_node().lines.as_slice()
    }

    pub fn selected_link(&self) -> Option<usize> {
        self.selected_link
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn move_lines(&mut self, delta: isize) {
        if self.lines().is_empty() {
            self.scroll = 0;
            return;
        }

        let max_scroll = self.lines().len().saturating_sub(1);
        self.scroll = if delta.is_negative() {
            self.scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.scroll.saturating_add(delta as usize).min(max_scroll)
        };
    }

    pub fn move_pages(&mut self, pages: isize) {
        self.move_lines(pages.saturating_mul(HELP_PAGE_STEP as isize));
    }

    pub fn move_half_pages(&mut self, pages: isize) {
        self.move_lines(pages.saturating_mul(HELP_HALF_PAGE_STEP as isize));
    }

    pub fn move_home(&mut self) {
        self.scroll = 0;
    }

    pub fn move_end(&mut self) {
        self.scroll = self.lines().len().saturating_sub(1);
    }

    pub fn select_next_link(&mut self) {
        let link_count = self.current_node().links.len();
        if link_count == 0 {
            self.selected_link = None;
            return;
        }

        self.selected_link = Some(match self.selected_link {
            Some(index) => (index + 1) % link_count,
            None => 0,
        });
        self.keep_selected_link_visible();
    }

    pub fn select_prev_link(&mut self) {
        let link_count = self.current_node().links.len();
        if link_count == 0 {
            self.selected_link = None;
            return;
        }

        self.selected_link = Some(match self.selected_link {
            Some(0) | None => link_count - 1,
            Some(index) => index - 1,
        });
        self.keep_selected_link_visible();
    }

    pub fn follow_selected_link(&mut self) -> bool {
        let Some(link_index) = self.selected_link else {
            return false;
        };
        let Some(target) = self.current_node().link_target(link_index) else {
            return false;
        };
        let Some(&target_node) = self.index_by_id.get(target) else {
            return false;
        };
        if target_node == self.current_node {
            return true;
        }

        self.history.push(self.current_node);
        self.current_node = target_node;
        self.scroll = 0;
        self.select_default_link();
        true
    }

    pub fn back(&mut self) -> bool {
        let Some(previous_node) = self.history.pop() else {
            return false;
        };
        self.current_node = previous_node;
        self.scroll = 0;
        self.select_default_link();
        true
    }

    pub fn open_index(&mut self) {
        self.open_topic(HELP_INDEX_ID, true);
    }

    pub fn open_next_node(&mut self) {
        if self.nodes.is_empty() {
            return;
        }
        let next = (self.current_node + 1) % self.nodes.len();
        self.history.push(self.current_node);
        self.current_node = next;
        self.scroll = 0;
        self.select_default_link();
    }

    pub fn open_prev_node(&mut self) {
        if self.nodes.is_empty() {
            return;
        }
        let previous = if self.current_node == 0 {
            self.nodes.len() - 1
        } else {
            self.current_node - 1
        };
        self.history.push(self.current_node);
        self.current_node = previous;
        self.scroll = 0;
        self.select_default_link();
    }

    fn current_node(&self) -> &HelpNode {
        &self.nodes[self.current_node]
    }

    fn open_topic(&mut self, topic_id: &str, push_history: bool) {
        let target = self.index_by_id.get(topic_id).copied().unwrap_or(0);
        if target == self.current_node {
            self.scroll = 0;
            self.select_default_link();
            return;
        }

        if push_history {
            self.history.push(self.current_node);
        }
        self.current_node = target;
        self.scroll = 0;
        self.select_default_link();
    }

    fn select_default_link(&mut self) {
        self.selected_link = (!self.current_node().links.is_empty()).then_some(0);
    }

    fn keep_selected_link_visible(&mut self) {
        let Some(link_index) = self.selected_link else {
            return;
        };
        let Some(line) = self.current_node().link_line(link_index) else {
            return;
        };
        if line < self.scroll {
            self.scroll = line;
        }
    }
}

fn topic_for_context(context: KeyContext) -> &'static str {
    match context {
        KeyContext::FileManager | KeyContext::FileManagerXMap => "file-manager",
        KeyContext::Jobs => "jobs",
        KeyContext::FindResults => "find-results",
        KeyContext::Tree => "tree",
        KeyContext::Hotlist => "hotlist",
        KeyContext::Viewer | KeyContext::ViewerHex => "viewer",
        KeyContext::Help => "help-viewer",
        _ => HELP_INDEX_ID,
    }
}

fn build_nodes() -> (Vec<HelpNode>, HashMap<String, usize>) {
    let index_by_id = HELP_NODE_SPECS
        .iter()
        .enumerate()
        .map(|(index, (id, _, _))| (id.to_string(), index))
        .collect::<HashMap<_, _>>();

    let nodes = HELP_NODE_SPECS
        .iter()
        .map(|(id, title, body)| parse_node(id, title, body))
        .collect::<Vec<_>>();
    (nodes, index_by_id)
}

fn parse_node(id: &str, title: &str, body: &str) -> HelpNode {
    let mut links = Vec::new();
    let lines = body
        .lines()
        .enumerate()
        .map(|(line_number, line)| parse_line(line, line_number, &mut links))
        .collect::<Vec<_>>();

    HelpNode {
        id: id.to_string(),
        title: title.to_string(),
        lines,
        links,
    }
}

fn parse_line(line: &str, line_number: usize, links: &mut Vec<HelpLink>) -> HelpLine {
    let mut spans = Vec::new();
    let mut remaining = line;

    loop {
        let Some(open_index) = remaining.find('[') else {
            if spans.is_empty() || !remaining.is_empty() {
                spans.push(HelpSpan::Text(remaining.to_string()));
            }
            break;
        };

        let (before, after_open) = remaining.split_at(open_index);
        if !before.is_empty() {
            spans.push(HelpSpan::Text(before.to_string()));
        }

        let Some(close_label_index) = after_open.find("](") else {
            spans.push(HelpSpan::Text(after_open.to_string()));
            break;
        };
        if close_label_index == 1 {
            spans.push(HelpSpan::Text(after_open.to_string()));
            break;
        }

        let after_label = &after_open[close_label_index + 2..];
        let Some(close_target_index) = after_label.find(')') else {
            spans.push(HelpSpan::Text(after_open.to_string()));
            break;
        };

        let label = &after_open[1..close_label_index];
        let target = after_label[..close_target_index].trim();
        let link_index = links.len();
        links.push(HelpLink {
            target: target.to_string(),
            line: line_number,
        });
        spans.push(HelpSpan::Link {
            label: label.to_string(),
            link_index,
        });
        remaining = &after_label[close_target_index + 1..];
    }

    HelpLine { spans }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flatten_help_lines(lines: &[HelpLine]) -> String {
        let mut text = String::new();
        for line in lines {
            for span in &line.spans {
                match span {
                    HelpSpan::Text(value) => text.push_str(value),
                    HelpSpan::Link { label, .. } => text.push_str(label),
                }
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn parses_links_and_keeps_order() {
        let node = parse_node(
            "node",
            "Node",
            "See [first](a) and [second](b).\nThen [third](c).",
        );
        assert_eq!(node.links.len(), 3);
        assert_eq!(node.links[0].target, "a");
        assert_eq!(node.links[1].target, "b");
        assert_eq!(node.links[2].target, "c");
        assert_eq!(node.links[0].line, 0);
        assert_eq!(node.links[2].line, 1);
    }

    #[test]
    fn navigation_follows_links_and_supports_back() {
        let mut help = HelpState::for_context(KeyContext::FileManager);
        assert_eq!(help.current_id(), "file-manager");

        help.open_index();
        assert_eq!(help.current_id(), HELP_INDEX_ID);

        help.select_next_link();
        assert!(help.follow_selected_link());
        assert_ne!(help.current_id(), HELP_INDEX_ID);
        assert!(help.back());
        assert_eq!(help.current_id(), HELP_INDEX_ID);
    }

    #[test]
    fn file_manager_help_includes_startup_shortcuts() {
        let help = HelpState::for_context(KeyContext::FileManager);
        assert_eq!(help.current_id(), "file-manager");

        let content = flatten_help_lines(help.lines());
        assert!(content.contains("Tab switch panel"));
        assert!(content.contains("Ctrl-X ! (or Alt/Ctrl-P) open external panelize"));
        assert!(content.contains("q/F10 quit"));
    }
}
