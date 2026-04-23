#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use gpui::{
    AnyElement, App, AppContext as _, Application, AssetSource, ClipboardItem, Context, Corner,
    DismissEvent, Entity, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement as _, Pixels, Point,
    Render, ScrollDelta, ScrollStrategy, ScrollWheelEvent, SharedString, Size,
    StatefulInteractiveElement as _, Styled, StyledImage as _, Subscription, Window, WindowOptions,
    anchored, deferred, div, img, point, prelude::FluentBuilder, px, size,
};
use gpui_component::{
    ActiveTheme as _, IconName, Root, Sizable as _, Theme, ThemeRegistry, TitleBar,
    VirtualListScrollHandle,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputState},
    list::ListItem,
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    resizable::{h_resizable, resizable_panel, v_resizable},
    scroll::{Scrollbar, ScrollbarHandle as _, ScrollbarShow},
    tab::{Tab, TabBar},
    table::{Column, ColumnSort, Table, TableDelegate, TableEvent, TableState},
    tree::{TreeItem, TreeState, tree},
    v_flex, v_virtual_list,
};

use chrono::{Local, TimeZone};
use log::warn;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read as _;
use std::path::PathBuf;
use std::rc::Rc;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

struct AppAssets {
    root: PathBuf,
}

impl AppAssets {
    fn new() -> Self {
        Self {
            root: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        }
    }

    fn resolve_path(&self, path: &str) -> Option<PathBuf> {
        if path.is_empty() || path.contains("..") || path.starts_with('/') || path.starts_with('\\')
        {
            return None;
        }
        Some(self.root.join(path))
    }
}

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let Some(full_path) = self.resolve_path(path) else {
            return Ok(None);
        };
        match fs::read(full_path) {
            Ok(bytes) => Ok(Some(Cow::Owned(bytes))),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let dir = if path.is_empty() {
            self.root.clone()
        } else {
            match self.resolve_path(path) {
                Some(p) => p,
                None => return Ok(Vec::new()),
            }
        };

        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err.into()),
        };

        let mut items = Vec::new();
        for entry in entries {
            let entry = entry?;
            items.push(entry.file_name().to_string_lossy().to_string().into());
        }
        Ok(items)
    }
}

// ── bbsmenu.json データ構造 ──────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct BoardEntry {
    pub board_name: String,
    pub url: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub directory_name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CategoryEntry {
    pub category_name: String,
    #[serde(default)]
    pub category_content: Vec<BoardEntry>,
}

#[derive(Debug, Deserialize)]
struct BbsMenu {
    pub menu_list: Vec<CategoryEntry>,
}

/// 互換のため、旧キャッシュ形式（板フラット配列）も受ける
#[derive(Debug, Clone, Deserialize)]
struct LegacyFlatBoardEntry {
    #[serde(alias = "category_name", alias = "category")]
    pub category_name: String,
    #[serde(alias = "board_name", alias = "title")]
    pub board_name: String,
    #[serde(alias = "url", alias = "id")]
    pub url: String,
    #[serde(default)]
    pub directory_name: String,
}

#[derive(Clone)]
struct ThreadTab {
    board_name: SharedString,
    board_url: SharedString,
    threads: Vec<ThreadItem>,
    loading: bool,
    error: Option<SharedString>,
}

#[derive(Clone)]
struct ThreadItem {
    dat_file: SharedString,
    title: SharedString,
    response_count: usize,
}

#[derive(Clone)]
struct ThreadTableRow {
    number: usize,
    board_url: SharedString,
    dat_file: SharedString,
    title: SharedString,
    response_count: usize,
    created_at: SharedString,
    momentum_per_day: SharedString,
}

#[derive(Clone)]
struct ResponseItem {
    number: usize,
    name: SharedString,
    timestamp: SharedString,
    id: SharedString,
    body: SharedString,
}

struct ThreadTableDelegate {
    columns: Vec<Column>,
    rows: Vec<ThreadTableRow>,
    empty_message: SharedString,
}

impl ThreadTableDelegate {
    fn new() -> Self {
        Self {
            columns: vec![
                Column::new("no", "番号")
                    .width(px(64.))
                    .resizable(false)
                    .sortable(),
                Column::new("title", "タイトル").width(px(300.)).sortable(),
                Column::new("responses", "レス数")
                    .width(px(70.))
                    .text_right()
                    .sortable(),
                Column::new("created_at", "スレ立")
                    .width(px(170.))
                    .sortable(),
                Column::new("momentum", "勢い")
                    .width(px(70.))
                    .text_right()
                    .sortable(),
            ],
            rows: Vec::new(),
            empty_message: "左の板一覧から板を選択してください".into(),
        }
    }

    fn set_rows(&mut self, rows: Vec<ThreadTableRow>, empty_message: SharedString) {
        self.rows = rows;
        self.empty_message = empty_message;
    }

    fn row(&self, row_ix: usize) -> Option<&ThreadTableRow> {
        self.rows.get(row_ix)
    }

    fn cell_text(row: &ThreadTableRow, key: &str) -> SharedString {
        match key {
            "no" => row.number.to_string().into(),
            "title" => row.title.clone(),
            "responses" => row.response_count.to_string().into(),
            "created_at" => row.created_at.clone(),
            "momentum" => row.momentum_per_day.clone(),
            _ => "".into(),
        }
    }
}

impl TableDelegate for ThreadTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> &Column {
        &self.columns[col_ix]
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        let key = self
            .columns
            .get(col_ix)
            .map(|c| c.key.as_ref().to_string())
            .unwrap_or_default();
        self.rows.sort_by(|a, b| {
            let ord = match key.as_str() {
                "no" => a.number.cmp(&b.number),
                "title" => a.title.as_ref().cmp(b.title.as_ref()),
                "responses" => a.response_count.cmp(&b.response_count),
                "created_at" => a.created_at.as_ref().cmp(b.created_at.as_ref()),
                "momentum" => {
                    let av = a.momentum_per_day.parse::<f64>().unwrap_or(0.0);
                    let bv = b.momentum_per_day.parse::<f64>().unwrap_or(0.0);
                    av.partial_cmp(&bv).unwrap_or(std::cmp::Ordering::Equal)
                }
                _ => std::cmp::Ordering::Equal,
            };

            match sort {
                ColumnSort::Ascending => ord,
                ColumnSort::Descending => ord.reverse(),
                ColumnSort::Default => ord,
            }
        });
    }

    fn move_column(
        &mut self,
        col_ix: usize,
        to_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        if col_ix >= self.columns.len() || to_ix >= self.columns.len() || col_ix == to_ix {
            return;
        }
        let moved = self.columns.remove(col_ix);
        self.columns.insert(to_ix, moved);
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(row) = self.rows.get(row_ix) else {
            return div();
        };

        let Some(column) = self.columns.get(col_ix) else {
            return div();
        };

        let text = Self::cell_text(row, column.key.as_ref());

        div().size_full().items_center().child(text)
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        h_flex()
            .size_full()
            .justify_center()
            .items_center()
            .text_color(cx.theme().muted_foreground)
            .child(self.empty_message.clone())
    }
}

#[derive(Clone, Copy)]
enum BodyTokenKind {
    Plain,
    Url,
    Anchor,
}

fn parse_bbsmenu_content(content: &str) -> Result<Vec<CategoryEntry>, String> {
    if let Ok(menu) = serde_json::from_str::<BbsMenu>(content) {
        return Ok(menu.menu_list);
    }

    if let Ok(categories) = serde_json::from_str::<Vec<CategoryEntry>>(content) {
        return Ok(categories);
    }

    if let Ok(flat_boards) = serde_json::from_str::<Vec<LegacyFlatBoardEntry>>(content) {
        let mut grouped: BTreeMap<String, Vec<BoardEntry>> = BTreeMap::new();
        for board in flat_boards {
            grouped
                .entry(board.category_name)
                .or_default()
                .push(BoardEntry {
                    board_name: board.board_name,
                    url: board.url,
                    directory_name: board.directory_name,
                });
        }

        let categories = grouped
            .into_iter()
            .map(|(category_name, category_content)| CategoryEntry {
                category_name,
                category_content,
            })
            .collect();
        return Ok(categories);
    }

    Err("bbsmenu.json の形式が想定外です".to_string())
}

fn fetch_bbsmenu_network() -> Result<String, String> {
    ureq::get("https://menu.5ch.io/bbsmenu.json")
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())
}

fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let mut reader = ureq::get(url)
        .call()
        .map_err(|e| e.to_string())?
        .into_reader();
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

enum DatRangeFetchResult {
    Full(Vec<u8>),
    Append(Vec<u8>),
    NotModified,
}

fn fetch_dat_bytes_with_range(
    url: &str,
    current_size: usize,
) -> Result<DatRangeFetchResult, String> {
    let request = ureq::get(url).set("Range", &format!("bytes={current_size}-"));
    let response = match request.call() {
        Ok(resp) => resp,
        Err(ureq::Error::Status(302, _resp)) => return Ok(DatRangeFetchResult::NotModified),
        Err(ureq::Error::Status(416, _resp)) => return Ok(DatRangeFetchResult::NotModified),
        Err(ureq::Error::Status(_status, _resp)) => {
            // Range が拒否された場合はフル取得へフォールバック
            return Ok(DatRangeFetchResult::Full(fetch_bytes(url)?));
        }
        Err(err) => return Err(err.to_string()),
    };

    let status = response.status();
    let mut reader = response.into_reader();
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    match status {
        200 => Ok(DatRangeFetchResult::Full(bytes)),
        206 => Ok(DatRangeFetchResult::Append(bytes)),
        _ => Ok(DatRangeFetchResult::Full(fetch_bytes(url)?)),
    }
}

/// 5chのテキストをShift_JISとしてデコードする共通処理
fn decode_5ch_bytes(bytes: &[u8]) -> String {
    let (text, _, _) = encoding_rs::SHIFT_JIS.decode(bytes);
    text.into_owned()
}

fn board_dir_from_url(board_url: &str) -> String {
    let after_scheme = board_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(board_url);

    let mut parts = after_scheme.split('/');
    let _host = parts.next();
    let board_dir = parts
        .find(|segment| !segment.is_empty())
        .unwrap_or("unknown");

    board_dir
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn subject_cache_path(board_url: &str) -> Result<PathBuf, String> {
    let board_dir = board_dir_from_url(board_url);

    Ok(dirs::config_dir()
        .ok_or_else(|| "ホームディレクトリが見つかりません".to_string())?
        .join("gooey")
        .join("log")
        .join("5ch")
        .join(board_dir)
        .join("subject.txt"))
}

fn subject_url(board_url: &str) -> String {
    if board_url.ends_with('/') {
        format!("{board_url}subject.txt")
    } else {
        format!("{board_url}/subject.txt")
    }
}

fn parse_subject_content(content: &str) -> Vec<ThreadItem> {
    content
        .lines()
        .filter_map(|line| {
            let (dat, title_part) = line.split_once("<>")?;
            let dat = dat.trim();
            let title_part = title_part.trim();

            let (title, count) = if let Some((left, right)) = title_part.rsplit_once(" (") {
                let count = right.strip_suffix(')').unwrap_or(right).trim();
                (left.trim(), count.parse::<usize>().unwrap_or(0))
            } else {
                (title_part, 0)
            };
            let decoded_title = decode_numeric_char_references(&decode_basic_html_entities(title));

            Some(ThreadItem {
                dat_file: dat.to_string().into(),
                title: decoded_title.into(),
                response_count: count,
            })
        })
        .collect()
}

fn parse_dat_unix_seconds(dat_file: &str) -> Option<i64> {
    let raw = dat_file.strip_suffix(".dat").unwrap_or(dat_file).trim();
    let unix = raw.parse::<i64>().ok()?;
    if unix > 0 { Some(unix) } else { None }
}

fn format_thread_created_at(dat_file: &str) -> SharedString {
    let Some(unix) = parse_dat_unix_seconds(dat_file) else {
        return "-".into();
    };
    let Some(dt) = Local.timestamp_opt(unix, 0).single() else {
        return "-".into();
    };
    dt.format("%Y/%m/%d %H:%M").to_string().into()
}

fn format_thread_momentum_per_day(response_count: usize, dat_file: &str) -> SharedString {
    let Some(unix) = parse_dat_unix_seconds(dat_file) else {
        return "0.0".into();
    };

    let now = Local::now().timestamp();
    let elapsed_seconds = (now - unix).max(1) as f64;
    let momentum = response_count as f64 * 86_400.0 / elapsed_seconds;
    format!("{momentum:.1}").into()
}

fn load_subject_lines(board_url: &str) -> Result<Vec<ThreadItem>, String> {
    let cache_path = subject_cache_path(board_url)?;
    if cache_path.exists() {
        let raw = std::fs::read(&cache_path).map_err(|e| e.to_string())?;
        let content = decode_5ch_bytes(&raw);
        let rows = parse_subject_content(&content);
        if !rows.is_empty() {
            return Ok(rows);
        }
    }

    let raw = fetch_bytes(&subject_url(board_url))?;

    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&cache_path, &raw);

    let content = decode_5ch_bytes(&raw);

    let rows = parse_subject_content(&content);
    if rows.is_empty() {
        return Err("subject.txt を解析できませんでした".to_string());
    }

    Ok(rows)
}

fn load_subject_lines_force_refresh(board_url: &str) -> Result<Vec<ThreadItem>, String> {
    let cache_path = subject_cache_path(board_url)?;
    let raw = fetch_bytes(&subject_url(board_url))?;

    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&cache_path, &raw);

    let content = decode_5ch_bytes(&raw);
    let rows = parse_subject_content(&content);
    if rows.is_empty() {
        return Err("subject.txt を解析できませんでした".to_string());
    }

    Ok(rows)
}

fn dat_cache_path(board_url: &str, dat_file: &str) -> Result<PathBuf, String> {
    let board_dir = board_dir_from_url(board_url);
    let dat_file = dat_file
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();

    Ok(dirs::config_dir()
        .ok_or_else(|| "ホームディレクトリが見つかりません".to_string())?
        .join("gooey")
        .join("log")
        .join("5ch")
        .join(board_dir)
        .join("dat")
        .join(dat_file))
}

fn dat_url(board_url: &str, dat_file: &str) -> String {
    let dat_file = if dat_file.ends_with(".dat") {
        dat_file.to_string()
    } else {
        format!("{dat_file}.dat")
    };

    if board_url.ends_with('/') {
        format!("{board_url}dat/{dat_file}")
    } else {
        format!("{board_url}/dat/{dat_file}")
    }
}

fn decode_basic_html_entities(input: &str) -> String {
    input
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn decode_numeric_char_references(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut i = 0;

    while i < input.len() {
        let rest = &input[i..];

        if !rest.starts_with("&#") {
            if let Some(ch) = rest.chars().next() {
                out.push(ch);
                i += ch.len_utf8();
            } else {
                break;
            }
            continue;
        }

        let num_start = i + 2;
        let Some(semicolon_rel) = input[num_start..].find(';') else {
            out.push('&');
            i += 1;
            continue;
        };
        let num_end = num_start + semicolon_rel;
        let token = &input[num_start..num_end];

        let parsed = if let Some(hex) = token.strip_prefix('x').or_else(|| token.strip_prefix('X'))
        {
            u32::from_str_radix(hex, 16).ok()
        } else {
            token.parse::<u32>().ok()
        };

        if let Some(codepoint) = parsed.and_then(char::from_u32) {
            out.push(codepoint);
            i = num_end + 1;
        } else {
            out.push('&');
            i += 1;
        }
    }

    out
}

fn decode_inline_html_text(input: &str) -> String {
    decode_numeric_char_references(&decode_basic_html_entities(input))
}

fn parse_response_name_html(input: &str) -> Vec<(bool, String)> {
    // 5ch の名前欄は全体が <b>...</b> で囲われた後に描画されるため、
    // 生の名前欄文字列は「太字状態で開始」し、途中の </b> / <b> で重みを切り替える。
    let mut segments = Vec::new();
    let mut cursor = input;
    let mut is_bold = true;

    while !cursor.is_empty() {
        let open_ix = cursor.find("<b>");
        let close_ix = cursor.find("</b>");
        let next_tag = match (open_ix, close_ix) {
            (Some(open), Some(close)) if open < close => Some((open, "<b>", true)),
            (Some(_open), Some(close)) => Some((close, "</b>", false)),
            (Some(open), None) => Some((open, "<b>", true)),
            (None, Some(close)) => Some((close, "</b>", false)),
            (None, None) => None,
        };

        let Some((tag_ix, tag, next_bold)) = next_tag else {
            let decoded = decode_inline_html_text(cursor);
            if !decoded.is_empty() {
                segments.push((is_bold, decoded));
            }
            break;
        };

        if tag_ix > 0 {
            let decoded = decode_inline_html_text(&cursor[..tag_ix]);
            if !decoded.is_empty() {
                segments.push((is_bold, decoded));
            }
        }

        cursor = &cursor[tag_ix + tag.len()..];
        is_bold = next_bold;
    }

    segments
}

fn response_name_plain_text(input: &str) -> String {
    parse_response_name_html(input)
        .into_iter()
        .map(|(_, segment)| segment)
        .collect::<String>()
}

fn split_response_name_tokens(input: &str) -> Vec<(bool, String)> {
    let mut tokens = Vec::new();

    for (is_bold, segment) in parse_response_name_html(input) {
        for token in segment.split_inclusive(char::is_whitespace) {
            if !token.is_empty() {
                tokens.push((is_bold, token.to_string()));
            }
        }
    }

    tokens
}

fn strip_anchor_tags(input: &str) -> String {
    let mut out = String::new();
    let mut cursor = input;

    while let Some(a_start) = cursor.find("<a") {
        out.push_str(&cursor[..a_start]);

        let Some(open_end_rel) = cursor[a_start..].find('>') else {
            out.push_str(&cursor[a_start..]);
            return out;
        };
        let content_start = a_start + open_end_rel + 1;

        let Some(close_rel) = cursor[content_start..].find("</a>") else {
            out.push_str(&cursor[content_start..]);
            return out;
        };

        let content_end = content_start + close_rel;
        out.push_str(&cursor[content_start..content_end]);

        cursor = &cursor[content_end + 4..];
    }

    out.push_str(cursor);
    out
}

fn normalize_dat_body_html(body: &str) -> String {
    let with_newlines = body
        .replace("<br>", "\n")
        .replace("<br />", "\n")
        .replace("<br/>", "\n");
    let unwrapped_anchors = strip_anchor_tags(&with_newlines);
    let basic_decoded = decode_basic_html_entities(&unwrapped_anchors);
    decode_numeric_char_references(&basic_decoded)
}

fn parse_dat_content(content: &str) -> Vec<ResponseItem> {
    content
        .lines()
        .enumerate()
        .map(|(ix, line)| {
            let mut parts = line.splitn(5, "<>");
            let name = parts.next().unwrap_or("").trim();
            let _mail = parts.next().unwrap_or("").trim();
            let date = parts.next().unwrap_or("").trim();
            let body = parts.next().unwrap_or("").trim();
            let body = normalize_dat_body_html(body);

            let id_token = date
                .split_whitespace()
                .find(|token| token.starts_with("ID:"))
                .unwrap_or("ID:----");
            let timestamp = date.replace(id_token, "").trim().to_string();

            ResponseItem {
                number: ix + 1,
                name: name.to_string().into(),
                timestamp: timestamp.into(),
                id: id_token.to_string().into(),
                body: body.into(),
            }
        })
        .collect()
}

fn load_dat_lines(board_url: &str, dat_file: &str) -> Result<Vec<ResponseItem>, String> {
    let cache_path = dat_cache_path(board_url, dat_file)?;
    if cache_path.exists() {
        let raw = std::fs::read(&cache_path).map_err(|e| e.to_string())?;
        let content = decode_5ch_bytes(&raw);
        let rows = parse_dat_content(&content);
        if !rows.is_empty() {
            return Ok(rows);
        }
    }

    let raw = fetch_bytes(&dat_url(board_url, dat_file))?;
    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&cache_path, &raw);

    let content = decode_5ch_bytes(&raw);
    let rows = parse_dat_content(&content);
    if rows.is_empty() {
        return Err("dat を解析できませんでした".to_string());
    }
    Ok(rows)
}

fn load_dat_lines_force_refresh(
    board_url: &str,
    dat_file: &str,
) -> Result<Vec<ResponseItem>, String> {
    let cache_path = dat_cache_path(board_url, dat_file)?;
    let url = dat_url(board_url, dat_file);

    let mut cached_raw = if cache_path.exists() {
        std::fs::read(&cache_path).map_err(|e| e.to_string())?
    } else {
        Vec::new()
    };

    let raw = if cached_raw.is_empty() {
        fetch_bytes(&url)?
    } else {
        match fetch_dat_bytes_with_range(&url, cached_raw.len())? {
            DatRangeFetchResult::Full(full) => full,
            DatRangeFetchResult::Append(delta) => {
                if delta.is_empty() {
                    cached_raw
                } else {
                    // 5ch dat の差分取得は通常 LF(0x0A) から始まる。
                    // 異常な差分は破損防止のためフル再取得する。
                    if delta.first().copied() != Some(b'\n') {
                        fetch_bytes(&url)?
                    } else {
                        cached_raw.extend_from_slice(&delta);
                        cached_raw
                    }
                }
            }
            DatRangeFetchResult::NotModified => cached_raw,
        }
    };
    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&cache_path, &raw);

    let content = decode_5ch_bytes(&raw);
    let rows = parse_dat_content(&content);
    if rows.is_empty() {
        return Err("dat を解析できませんでした".to_string());
    }

    Ok(rows)
}

type PostCookieStore = BTreeMap<String, BTreeMap<String, String>>;

enum PostAttemptResult {
    Posted,
    NeedConfirm { feature: String, time: String },
}

fn post_cookie_store_path() -> Result<PathBuf, String> {
    Ok(dirs::config_dir()
        .ok_or_else(|| "ホームディレクトリが見つかりません".to_string())?
        .join("gooey")
        .join("post_cookie_store.json"))
}

fn load_post_cookie_store() -> PostCookieStore {
    let Ok(path) = post_cookie_store_path() else {
        return BTreeMap::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    serde_json::from_str::<PostCookieStore>(&text).unwrap_or_default()
}

fn save_post_cookie_store(store: &PostCookieStore) {
    let Ok(path) = post_cookie_store_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(store) {
        let _ = std::fs::write(path, json);
    }
}

fn parse_board_server(board_url: &str) -> Result<(String, String), String> {
    let after_scheme = board_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(board_url);
    let mut parts = after_scheme.split('/').filter(|s| !s.is_empty());
    let server = parts
        .next()
        .ok_or_else(|| "投稿先サーバーを判定できません".to_string())?
        .to_string();
    let board = parts
        .next()
        .ok_or_else(|| "投稿先板を判定できません".to_string())?
        .to_string();
    Ok((server, board))
}

fn parse_set_cookie_pair(raw: &str) -> Option<(String, String)> {
    let first = raw.split(';').next()?.trim();
    let (name, value) = first.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some((name.to_string(), value.trim().to_string()))
}

fn merge_set_cookie_values(cookies: &mut BTreeMap<String, String>, set_cookie_values: &[String]) {
    for raw in set_cookie_values {
        if let Some((name, value)) = parse_set_cookie_pair(raw) {
            // 5ch の投稿に不要な cookie は保存しない
            if name.eq_ignore_ascii_case("TAKO")
                || name.eq_ignore_ascii_case("BAN")
                || name.eq_ignore_ascii_case("__cfduid")
            {
                continue;
            }
            cookies.insert(name, value);
        }
    }
}

fn build_cookie_header(cookies: &BTreeMap<String, String>) -> Option<String> {
    if cookies.is_empty() {
        return None;
    }
    Some(
        cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

fn extract_hidden_input_value(html: &str, name: &str) -> Option<String> {
    let pats = [
        format!("name=\"{name}\""),
        format!("name='{name}'"),
        format!("name={name}"),
    ];

    for pat in pats {
        let mut cursor = 0;
        while let Some(rel) = html[cursor..].find(&pat) {
            let found = cursor + rel;
            let tag_start = html[..found].rfind('<')?;
            let tag_end_rel = html[found..].find('>')?;
            let tag_end = found + tag_end_rel;
            let tag = &html[tag_start..=tag_end];
            if let Some(vpos) = tag.find("value=") {
                let value_part = &tag[vpos + 6..];
                if let Some(rest) = value_part.strip_prefix('"') {
                    if let Some(end) = rest.find('"') {
                        return Some(rest[..end].to_string());
                    }
                } else if let Some(rest) = value_part.strip_prefix('\'') {
                    if let Some(end) = rest.find('\'') {
                        return Some(rest[..end].to_string());
                    }
                } else {
                    let end = value_part
                        .find(|c: char| c.is_ascii_whitespace() || c == '>')
                        .unwrap_or(value_part.len());
                    return Some(value_part[..end].to_string());
                }
            }
            cursor = found + pat.len();
        }
    }

    None
}

fn post_5ch(
    board_url: &str,
    dat_file: &str,
    name: &str,
    mail: &str,
    message: &str,
    use_sage: bool,
    pending_confirm: Option<(String, String)>,
) -> Result<PostAttemptResult, String> {
    let (server, board) = parse_board_server(board_url)?;
    let key = dat_file
        .strip_suffix(".dat")
        .unwrap_or(dat_file)
        .trim()
        .to_string();
    if key.is_empty() {
        return Err("スレッドキーを判定できません".to_string());
    }

    let mut post_cookie_store = load_post_cookie_store();
    let cookie_scope = format!("{server}/{board}");
    let mut scope_cookies = post_cookie_store.remove(&cookie_scope).unwrap_or_default();

    let now_unix = Local::now().timestamp();
    let post_time = (now_unix - 60).max(1).to_string();
    let mut post_mail = mail.trim().to_string();
    if use_sage && post_mail.is_empty() {
        post_mail = "sage".to_string();
    }

    let mut form = vec![
        ("submit".to_string(), "書き込む".to_string()),
        ("time".to_string(), post_time),
        ("oekaki_thread1".to_string(), "".to_string()),
        ("bbs".to_string(), board.clone()),
        ("key".to_string(), key),
        ("FROM".to_string(), name.to_string()),
        ("mail".to_string(), post_mail),
        ("MESSAGE".to_string(), message.to_string()),
    ];

    if let Some((feature, time)) = pending_confirm {
        form.retain(|(k, _)| k != "time" && k != "submit");
        form.push(("feature".to_string(), feature));
        form.push(("time".to_string(), time));
        form.push((
            "submit".to_string(),
            "上記全てを承諾して書き込む".to_string(),
        ));
    }

    let form_refs: Vec<(&str, &str)> = form.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

    let post_url = format!("https://{server}/test/bbs.cgi");
    let referer = post_url.clone();

    let mut request = ureq::post(&post_url)
        .set("referer", &referer)
        .set(
            "content-type",
            "application/x-www-form-urlencoded; charset=UTF-8",
        )
        .set(
            "accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .set("accept-language", "ja-JP,ja;q=0.8")
        .set("user-agent", "Monazilla/1.00 Gooey/0.1");

    if let Some(cookie_header) = build_cookie_header(&scope_cookies) {
        request = request.set("cookie", &cookie_header);
    }

    let response = match request.send_form(&form_refs) {
        Ok(resp) => resp,
        Err(ureq::Error::Status(_, resp)) => resp,
        Err(err) => return Err(format!("投稿通信に失敗しました: {err}")),
    };

    let set_cookie_values = response
        .all("set-cookie")
        .iter()
        .map(|v| (*v).to_string())
        .collect::<Vec<_>>();
    if !set_cookie_values.is_empty() {
        merge_set_cookie_values(&mut scope_cookies, &set_cookie_values);
        post_cookie_store.insert(cookie_scope.clone(), scope_cookies.clone());
        save_post_cookie_store(&post_cookie_store);
    }

    let mut reader = response.into_reader();
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let body = decode_5ch_bytes(&bytes);

    if body.contains("書きこみが終わりました")
        || body.contains("書き込みが終わりました")
        || body.contains("書きこみました")
    {
        return Ok(PostAttemptResult::Posted);
    }

    let feature =
        extract_hidden_input_value(&body, "feature").filter(|v| v.starts_with("confirmed:"));
    let time = extract_hidden_input_value(&body, "time");
    if let (Some(feature), Some(time)) = (feature, time) {
        return Ok(PostAttemptResult::NeedConfirm { feature, time });
    }

    Err("投稿に失敗しました。本文かCookie、または投稿条件を確認してください".to_string())
}

/// キャッシュから読み込む、なければネットワーク取得してキャッシュに保存する
fn load_bbsmenu() -> Result<Vec<CategoryEntry>, String> {
    let cache_path = dirs::config_dir()
        .ok_or_else(|| "ホームディレクトリが見つかりません".to_string())?
        .join("gooey")
        .join("bbsmenu.json");

    if cache_path.exists() {
        let content = std::fs::read_to_string(&cache_path).map_err(|e| e.to_string())?;
        if let Ok(categories) = parse_bbsmenu_content(&content) {
            return Ok(categories);
        }
    }

    let content = fetch_bbsmenu_network()?;

    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&cache_path, &content);

    parse_bbsmenu_content(&content)
}

/// カテゴリ一覧を TreeItem 階層に変換する
fn build_tree_items(
    categories: &[CategoryEntry],
    expanded_categories: &BTreeSet<String>,
) -> Vec<TreeItem> {
    categories
        .iter()
        .map(|cat| {
            let children: Vec<TreeItem> = cat
                .category_content
                .iter()
                .map(|board| TreeItem::new(board.url.clone(), board.board_name.clone()))
                .collect();
            TreeItem::new(cat.category_name.clone(), cat.category_name.clone())
                .expanded(expanded_categories.contains(&cat.category_name))
                .children(children)
        })
        .collect()
}

// ── アプリ状態 ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionTableColumn {
    key: String,
    width_px: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionBoardTab {
    board_name: String,
    board_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionThreadRef {
    board_url: String,
    dat_file: String,
    title: String,
    #[serde(default)]
    top_response_number: Option<usize>,
    #[serde(default)]
    bottom_response_number: Option<usize>,
    #[serde(default)]
    was_at_bottom: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionState {
    layout_mode: LayoutMode,
    active_thread_tab: usize,
    active_response_tab: usize,
    opened_categories: Vec<String>,
    thread_tabs: Vec<SessionBoardTab>,
    #[serde(default)]
    response_tabs: Vec<SessionThreadRef>,
    current_thread: Option<SessionThreadRef>,
    thread_scroll_y: f32,
    #[serde(default)]
    workspace_sidebar_width: f32,
    #[serde(default)]
    center_split_size: f32,
    #[serde(default)]
    theme_name: Option<String>,
    #[serde(default)]
    thread_table_columns: Vec<SessionTableColumn>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            layout_mode: LayoutMode::Horizontal,
            active_thread_tab: 0,
            active_response_tab: 0,
            opened_categories: Vec::new(),
            thread_tabs: Vec::new(),
            response_tabs: Vec::new(),
            current_thread: None,
            thread_scroll_y: 0.0,
            thread_table_columns: Vec::new(),
            workspace_sidebar_width: 250.0,
            center_split_size: 360.0,
            theme_name: None,
        }
    }
}

#[derive(Clone, Debug)]
struct OpenThreadState {
    board_url: SharedString,
    dat_file: SharedString,
    title: SharedString,
    /// タブごとの読んだ位置（現在表示中の上端レス番号）
    top_response_number: Option<usize>,
    /// タブごとの読んだ位置（現在表示中の下端レス番号）
    bottom_response_number: Option<usize>,
    /// タブごとの読了状態（最下部まで読んでいたか）
    was_at_bottom: bool,
}

/// モーダルダイアログの内容を表現する enum
#[derive(Clone, Debug)]
enum Modal {
    /// 確認ダイアログ（「はい」「いいえ」ボタン）
    Confirmation {
        title: SharedString,
        message: SharedString,
        ok_label: SharedString,
        cancel_label: SharedString,
    },
    /// アラート（「OK」ボタンのみ）
    Alert {
        title: SharedString,
        message: SharedString,
    },
}

#[derive(Clone, Debug)]
struct PostConfirmState {
    board_url: SharedString,
    dat_file: SharedString,
    feature: SharedString,
    time: SharedString,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
enum LayoutMode {
    Horizontal,
    Vertical,
    Single,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GesturePane {
    ThreadList,
    ResponseList,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GestureDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Debug)]
struct GestureTrackingState {
    pane: GesturePane,
    last: Point<Pixels>,
    trail: Vec<GestureDirection>,
}

#[derive(Default)]
struct PaneContextMenuState {
    menu_view: Option<Entity<PopupMenu>>,
    position: Point<Pixels>,
    open: bool,
    dismiss_subscription: Option<Subscription>,
}

#[derive(Clone, Debug)]
enum ImagePreviewStatus {
    Loading,
    Ready { local_path: PathBuf },
    Error { message: SharedString },
}

#[derive(Clone, Debug)]
struct ImagePreviewState {
    request_generation: u64,
    url: SharedString,
    position: Point<Pixels>,
    status: ImagePreviewStatus,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MouseGestureCommand {
    None,
    ScrollTop,
    ScrollBottom,
    Refresh,
    Close,
    TabPrev,
    TabNext,
}

impl Default for MouseGestureCommand {
    fn default() -> Self {
        MouseGestureCommand::None
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MouseGesturePaneBindings {
    #[serde(default)]
    commands: BTreeMap<String, MouseGestureCommand>,
}

impl Default for MouseGesturePaneBindings {
    fn default() -> Self {
        let mut commands = BTreeMap::new();
        commands.insert("u".to_string(), MouseGestureCommand::ScrollTop);
        commands.insert("d".to_string(), MouseGestureCommand::ScrollBottom);
        commands.insert("l".to_string(), MouseGestureCommand::TabPrev);
        commands.insert("r".to_string(), MouseGestureCommand::TabNext);
        commands.insert("ud".to_string(), MouseGestureCommand::Refresh);
        commands.insert("dr".to_string(), MouseGestureCommand::Close);
        commands.insert("udr".to_string(), MouseGestureCommand::Close);
        Self { commands }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MouseGestureSettings {
    #[serde(default = "default_mouse_gesture_enabled")]
    enabled: bool,
    #[serde(default = "default_mouse_gesture_threshold_px")]
    min_distance_px: f32,
    #[serde(default)]
    thread: MouseGesturePaneBindings,
    #[serde(default)]
    response: MouseGesturePaneBindings,
}

impl Default for MouseGestureSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            min_distance_px: 30.0,
            thread: MouseGesturePaneBindings::default(),
            response: MouseGesturePaneBindings::default(),
        }
    }
}

fn default_mouse_gesture_enabled() -> bool {
    true
}

fn default_mouse_gesture_threshold_px() -> f32 {
    30.0
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum KeyBindingCommand {
    None,
    PageUp,
    PageDown,
    ScrollTop,
    ScrollBottom,
    Refresh,
    Close,
}

impl Default for KeyBindingCommand {
    fn default() -> Self {
        KeyBindingCommand::None
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct KeyBindingPaneBindings {
    #[serde(default)]
    page_up: KeyBindingCommand,
    #[serde(default)]
    page_down: KeyBindingCommand,
    #[serde(default)]
    home: KeyBindingCommand,
    #[serde(default)]
    end: KeyBindingCommand,
    #[serde(default)]
    f5: KeyBindingCommand,
    #[serde(default)]
    ctrl_r: KeyBindingCommand,
    #[serde(default)]
    ctrl_w: KeyBindingCommand,
}

impl Default for KeyBindingPaneBindings {
    fn default() -> Self {
        Self {
            page_up: KeyBindingCommand::PageUp,
            page_down: KeyBindingCommand::PageDown,
            home: KeyBindingCommand::ScrollTop,
            end: KeyBindingCommand::ScrollBottom,
            f5: KeyBindingCommand::Refresh,
            ctrl_r: KeyBindingCommand::Refresh,
            ctrl_w: KeyBindingCommand::Close,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct KeyBindingSettings {
    #[serde(default = "default_key_binding_enabled")]
    enabled: bool,
    #[serde(default)]
    thread: KeyBindingPaneBindings,
    #[serde(default)]
    response: KeyBindingPaneBindings,
}

impl Default for KeyBindingSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            thread: KeyBindingPaneBindings::default(),
            response: KeyBindingPaneBindings::default(),
        }
    }
}

fn default_key_binding_enabled() -> bool {
    true
}

struct FiveChLayout {
    layout_mode: LayoutMode,
    active_thread_tab: usize,
    active_response_tab: usize,
    thread_scroll_handle: VirtualListScrollHandle,
    thread_table_state: Option<Entity<TableState<ThreadTableDelegate>>>,
    thread_table_subscription: Option<Subscription>,
    thread_table_source_signature: u64,
    response_scroll_handle: VirtualListScrollHandle,
    response_virtual_body_width_px: f32,
    tree_state: Entity<TreeState>,
    thread_tabs: Vec<ThreadTab>,
    response_tabs: Vec<OpenThreadState>,
    responses: Vec<ResponseItem>,
    board_to_category: BTreeMap<String, String>,
    opened_categories: BTreeSet<String>,
    current_thread: Option<OpenThreadState>,
    pending_restore_thread_scroll_y: Option<f32>,
    pending_restore_thread_table_columns: Option<Vec<SessionTableColumn>>,
    pending_restore_response_numbers: Option<(Option<usize>, Option<usize>)>,
    pending_restore_response_scroll_offset_y: Option<f32>,
    pending_restore_response_to_bottom: bool,
    pending_restore_session: Option<SessionState>,
    last_thread_scroll_y: f32,
    current_response_top_number: Option<usize>,
    current_response_bottom_number: Option<usize>,
    current_response_was_at_bottom: bool,
    response_new_marker_from: Option<usize>,
    workspace_sidebar_width: f32,
    center_split_size: f32,
    pending_restore_theme: Option<String>,
    current_theme_name: Option<String>,
    response_composer_open: bool,
    response_form_name_state: Option<Entity<InputState>>,
    response_form_mail_state: Option<Entity<InputState>>,
    response_form_body_state: Option<Entity<InputState>>,
    response_form_sage: bool,
    response_submit_hint: Option<SharedString>,
    post_confirm_state: Option<PostConfirmState>,
    modal_state: Option<Modal>,
    post_pending_confirm_empty_body: bool,
    mouse_gesture_settings: MouseGestureSettings,
    key_binding_settings: KeyBindingSettings,
    gesture_tracking: Option<GestureTrackingState>,
    gesture_trail_text: SharedString,
    gesture_clear_generation: u64,
    focused_pane: GesturePane,
    thread_pane_context_menu: PaneContextMenuState,
    response_row_context_menu: PaneContextMenuState,
    footer_status_hint: Option<SharedString>,
    pull_refresh_hint: Option<SharedString>,
    thread_pull_refresh_accum_px: f32,
    response_pull_refresh_accum_px: f32,
    response_content_height_px: f32,
    response_viewport_height_px: f32,
    image_preview_state: Option<ImagePreviewState>,
    image_preview_generation: u64,
    cached_thread_table_columns: Vec<SessionTableColumn>,
    session_save_debounce_generation: u64,
}

impl FiveChLayout {
    fn has_placeholder_response_row(&self) -> bool {
        self.responses.len() == 1 && self.responses.first().is_some_and(|res| res.number == 0)
    }

    fn has_loaded_response_rows(&self) -> bool {
        !self.responses.is_empty() && !self.has_placeholder_response_row()
    }

    fn is_url_like(token: &str) -> bool {
        let lower = token.to_ascii_lowercase();
        // 完全なプロトコル形式
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return true;
        }
        // h抜きプロトコル形式
        if lower.starts_with("ttp://") || lower.starts_with("ttps://") {
            return true;
        }
        // ドメイン+パス形式 (例: i.momicha.net/politics/...)
        // 最低限：ドットを含む + スラッシュを含む
        if token.contains('.') && token.contains('/') {
            // 最初のドットまでの部分がドメインラベル（英数字とハイフンのみ）なら URL 候補
            if let Some(slash_pos) = token.find('/') {
                let before_slash = &token[..slash_pos];
                // 最低1つのドットがあり、英数字とハイフン、ドットのみで構成
                if before_slash.contains('.')
                    && before_slash
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
                {
                    return true;
                }
            }
        }
        false
    }

    fn split_body_tokens(line: &str) -> Vec<(BodyTokenKind, String)> {
        let mut out = Vec::new();
        let mut current = String::new();

        for token in line.split_inclusive(char::is_whitespace) {
            let trimmed = token.trim_end();
            let whitespace = &token[trimmed.len()..];

            let push_current = |out: &mut Vec<(BodyTokenKind, String)>, current: &mut String| {
                if !current.is_empty() {
                    out.push((BodyTokenKind::Plain, std::mem::take(current)));
                }
            };

            if trimmed.is_empty() {
                current.push_str(whitespace);
                continue;
            }

            let is_url = Self::is_url_like(trimmed);
            let is_anchor = if let Some(rest) = trimmed.strip_prefix(">>") {
                !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
            } else {
                false
            };

            if is_url {
                push_current(&mut out, &mut current);
                out.push((BodyTokenKind::Url, format!("{trimmed}{whitespace}")));
            } else if is_anchor {
                push_current(&mut out, &mut current);
                out.push((BodyTokenKind::Anchor, format!("{trimmed}{whitespace}")));
            } else {
                current.push_str(trimmed);
                current.push_str(whitespace);
            }
        }

        if !current.is_empty() {
            out.push((BodyTokenKind::Plain, current));
        }
        out
    }

    fn normalize_url(url: &str) -> String {
        let trimmed = url.trim();
        // h抜きプロトコル形式を補正
        if trimmed.starts_with("ttp://") {
            format!("h{}", trimmed)
        } else if trimmed.starts_with("ttps://") {
            format!("h{}", trimmed)
        } else if !trimmed.starts_with("http://")
            && !trimmed.starts_with("https://")
            && (trimmed.contains('.') && trimmed.contains('/'))
        {
            // ドメイン形式 -> https:// を付与
            format!("https://{}", trimmed)
        } else {
            trimmed.to_string()
        }
    }

    fn parse_5ch_io_thread_url(url: &str) -> Option<(String, String)> {
        let normalized = Self::normalize_url(url);
        let without_fragment = normalized.split('#').next().unwrap_or(normalized.as_str());
        let without_query = without_fragment
            .split('?')
            .next()
            .unwrap_or(without_fragment);
        let (_, after_scheme) = without_query.split_once("://")?;

        let mut parts = after_scheme.split('/').filter(|s| !s.is_empty());
        let host = parts.next()?.to_ascii_lowercase();
        if host != "5ch.io" && !host.ends_with(".5ch.io") {
            return None;
        }

        let segments: Vec<&str> = parts.collect();
        if segments.len() < 4 || segments[0] != "test" || segments[1] != "read.cgi" {
            return None;
        }

        let board = segments[2].trim();
        if board.is_empty() {
            return None;
        }

        let mut thread_key = segments[3].trim();
        if let Some(stripped) = thread_key.strip_suffix(".dat") {
            thread_key = stripped;
        }
        if thread_key.is_empty() {
            return None;
        }

        let board_url = format!("https://{host}/{board}/");
        let dat_file = format!("{thread_key}.dat");
        Some((board_url, dat_file))
    }

    fn open_5ch_io_url_in_response_tab(&mut self, url: &str, cx: &mut Context<Self>) -> bool {
        let Some((board_url, dat_file)) = Self::parse_5ch_io_thread_url(url) else {
            return false;
        };

        let title = self
            .thread_tabs
            .iter()
            .find(|tab| tab.board_url.as_ref() == board_url)
            .and_then(|tab| {
                tab.threads
                    .iter()
                    .find(|thread| thread.dat_file.as_ref() == dat_file)
                    .map(|thread| thread.title.clone())
            })
            .unwrap_or_else(|| format!("{} {}", board_dir_from_url(&board_url), dat_file).into());

        self.set_focused_pane(GesturePane::ResponseList);
        self.open_thread(board_url.into(), dat_file.into(), title, cx);
        true
    }

    fn extract_displayed_urls(body: &str) -> Vec<String> {
        let mut urls = Vec::new();
        for line in body.lines() {
            for (kind, token) in Self::split_body_tokens(line) {
                if !matches!(kind, BodyTokenKind::Url) {
                    continue;
                }
                let url = token.trim().to_string();
                if !url.is_empty() {
                    let normalized = Self::normalize_url(&url);
                    if !urls.iter().any(|u| u == &normalized) {
                        urls.push(normalized);
                    }
                }
            }
        }
        urls
    }

    fn image_preview_cache_path(url: &str) -> Result<PathBuf, String> {
        let mut hasher = DefaultHasher::new();
        url.hash(&mut hasher);
        let hash = hasher.finish();

        let lower = url.to_ascii_lowercase();
        let without_query = lower.split(['?', '#']).next().unwrap_or(lower.as_str());
        let ext = if without_query.ends_with(".jpeg") {
            "jpeg"
        } else if without_query.ends_with(".jpg") {
            "jpg"
        } else if without_query.ends_with(".png") {
            "png"
        } else if without_query.ends_with(".gif") {
            "gif"
        } else if without_query.ends_with(".webp") {
            "webp"
        } else if without_query.ends_with(".bmp") {
            "bmp"
        } else {
            "img"
        };

        Ok(dirs::config_dir()
            .ok_or_else(|| "ホームディレクトリが見つかりません".to_string())?
            .join("gooey")
            .join("image_cache")
            .join(format!("{hash:016x}.{ext}")))
    }

    fn is_image_preview_url(url: &str) -> bool {
        let normalized = Self::normalize_url(url);
        let lower = normalized.to_ascii_lowercase();
        let without_query = lower.split(['?', '#']).next().unwrap_or(lower.as_str());
        without_query.ends_with(".jpg")
            || without_query.ends_with(".jpeg")
            || without_query.ends_with(".png")
            || without_query.ends_with(".gif")
            || without_query.ends_with(".webp")
            || without_query.ends_with(".bmp")
    }

    fn download_image_preview(url: &str) -> Result<PathBuf, String> {
        let normalized = Self::normalize_url(url);
        let path = Self::image_preview_cache_path(&normalized)?;
        if path.exists() {
            return Ok(path);
        }

        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let bytes = fetch_bytes(&normalized)?;
        fs::write(&path, bytes).map_err(|e| e.to_string())?;
        Ok(path)
    }

    fn begin_image_preview_for_url(
        &mut self,
        url: String,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if !Self::is_image_preview_url(&url) {
            return;
        }

        if let Some(state) = self.image_preview_state.as_mut()
            && state.url.as_ref() == url
        {
            state.position = position;
            cx.notify();
            return;
        }

        self.image_preview_generation = self.image_preview_generation.wrapping_add(1);
        let request_generation = self.image_preview_generation;
        self.image_preview_state = Some(ImagePreviewState {
            request_generation,
            url: url.clone().into(),
            position,
            status: ImagePreviewStatus::Loading,
        });
        cx.notify();

        let view = cx.entity();
        cx.spawn(async move |_weak_self, cx| {
            let url_for_fetch = url.clone();
            let result = cx
                .background_executor()
                .spawn(async move { Self::download_image_preview(&url_for_fetch) })
                .await;

            _ = view.update(cx, |this, cx| {
                let Some(state) = this.image_preview_state.as_mut() else {
                    return;
                };
                if state.request_generation != request_generation {
                    return;
                }

                state.status = match result {
                    Ok(local_path) => ImagePreviewStatus::Ready { local_path },
                    Err(err) => ImagePreviewStatus::Error {
                        message: format!("画像取得失敗: {err}").into(),
                    },
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn update_image_preview_position(
        &mut self,
        url: &str,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.image_preview_state.as_mut()
            && state.url.as_ref() == url
        {
            state.position = position;
            cx.notify();
        }
    }

    fn clear_image_preview_for_url(&mut self, url: &str, cx: &mut Context<Self>) {
        let should_clear = self
            .image_preview_state
            .as_ref()
            .is_some_and(|state| state.url.as_ref() == url);
        if should_clear {
            self.image_preview_state = None;
            self.image_preview_generation = self.image_preview_generation.wrapping_add(1);
            cx.notify();
        }
    }

    fn open_url_in_external_browser(url: &str) {
        let _ = if cfg!(target_os = "windows") {
            std::process::Command::new("explorer").arg(url).spawn()
        } else if cfg!(target_os = "macos") {
            std::process::Command::new("open").arg(url).spawn()
        } else {
            std::process::Command::new("xdg-open").arg(url).spawn()
        };
    }

    fn estimate_body_wrap_lines(text: &str, content_width_px: f32) -> usize {
        // text_sm の実表示をざっくり 1セル=7.2px とみなす
        let cols_per_line = (content_width_px / 7.2).floor().max(8.0) as usize;
        text.lines()
            .map(|line| {
                let w = UnicodeWidthStr::width(line);
                w.max(1).div_ceil(cols_per_line)
            })
            .sum::<usize>()
            .max(1)
    }

    fn estimate_response_header_wrap_lines(res: &ResponseItem, content_width_px: f32) -> usize {
        // ヘッダも text_sm ベースで概算する。gap_2 をざっくり 2 文字分として積む。
        let cols_per_line = (content_width_px / 7.2).floor().max(8.0) as usize;
        let name = format!(
            "{:>4} {}",
            res.number,
            response_name_plain_text(res.name.as_ref())
        );
        let total_width = UnicodeWidthStr::width(name.as_str())
            + UnicodeWidthStr::width(res.timestamp.as_ref())
            + UnicodeWidthStr::width(res.id.as_ref())
            + 4;
        total_width.max(1).div_ceil(cols_per_line).max(1)
    }

    fn estimate_response_row_height_px(&self, res: &ResponseItem) -> f32 {
        let content_width_px = (self.response_virtual_body_width_px - 24.0).max(64.0);
        let header_line_count =
            Self::estimate_response_header_wrap_lines(res, content_width_px) as f32;
        let line_count = Self::estimate_body_wrap_lines(res.body.as_ref(), content_width_px) as f32;
        // fixed: py_1(8) + gap_1(4) + border(1)
        let fixed_px = 13.0;
        let header_height_px = header_line_count * 22.0;
        let header_line_gaps_px = (header_line_count - 1.0).max(0.0) * 8.0;
        // 本文コンテナは gap_0p5(=2px) を使っているため、改行数に応じた行間も積む
        let logical_lines = res.body.lines().count().max(1) as f32;
        let body_line_gaps_px = (logical_lines - 1.0).max(0.0) * 2.0;
        fixed_px + header_height_px + header_line_gaps_px + line_count * 22.0 + body_line_gaps_px
    }

    fn session_file_path() -> Result<PathBuf, String> {
        Ok(dirs::config_dir()
            .ok_or_else(|| "ホームディレクトリが見つかりません".to_string())?
            .join("gooey")
            .join("session.json"))
    }

    fn mouse_gesture_file_path() -> Result<PathBuf, String> {
        Ok(dirs::config_dir()
            .ok_or_else(|| "ホームディレクトリが見つかりません".to_string())?
            .join("gooey")
            .join("mousegesture.json"))
    }

    fn key_binding_file_path() -> Result<PathBuf, String> {
        Ok(dirs::config_dir()
            .ok_or_else(|| "ホームディレクトリが見つかりません".to_string())?
            .join("gooey")
            .join("keybinding.json"))
    }

    fn load_mouse_gesture_settings_from_disk() -> MouseGestureSettings {
        let default_settings = MouseGestureSettings::default();
        let Ok(path) = Self::mouse_gesture_file_path() else {
            return default_settings;
        };

        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(mut settings) = serde_json::from_str::<MouseGestureSettings>(&text) {
                if settings.thread.commands.is_empty() {
                    settings.thread = MouseGesturePaneBindings::default();
                }
                if settings.response.commands.is_empty() {
                    settings.response = MouseGesturePaneBindings::default();
                }
                return settings;
            }
        }

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&default_settings) {
            let _ = std::fs::write(path, json);
        }

        default_settings
    }

    fn save_mouse_gesture_settings_to_disk(&self) {
        let Ok(path) = Self::mouse_gesture_file_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.mouse_gesture_settings) {
            let _ = std::fs::write(path, json);
        }
    }

    fn load_key_binding_settings_from_disk() -> KeyBindingSettings {
        let default_settings = KeyBindingSettings::default();
        let Ok(path) = Self::key_binding_file_path() else {
            return default_settings;
        };

        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(settings) = serde_json::from_str::<KeyBindingSettings>(&text) {
                return settings;
            }
        }

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&default_settings) {
            let _ = std::fs::write(path, json);
        }

        default_settings
    }

    fn save_key_binding_settings_to_disk(&self) {
        let Ok(path) = Self::key_binding_file_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.key_binding_settings) {
            let _ = std::fs::write(path, json);
        }
    }

    fn load_session_from_disk() -> Option<SessionState> {
        let path = Self::session_file_path().ok()?;
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str::<SessionState>(&text).ok()
    }

    fn build_session_state(&self) -> SessionState {
        SessionState {
            layout_mode: self.layout_mode,
            active_thread_tab: self.active_thread_tab,
            active_response_tab: self.active_response_tab,
            opened_categories: self.opened_categories.iter().cloned().collect(),
            thread_tabs: self
                .thread_tabs
                .iter()
                .map(|tab| SessionBoardTab {
                    board_name: tab.board_name.to_string(),
                    board_url: tab.board_url.to_string(),
                })
                .collect(),
            response_tabs: self
                .response_tabs
                .iter()
                .enumerate()
                .map(|(i, tab)| {
                    // アクティブなタブは現在可視のレス番号を優先して保存する
                    let (top_response_number, bottom_response_number) =
                        if i == self.active_response_tab {
                            (
                                self.current_response_top_number,
                                self.current_response_bottom_number,
                            )
                        } else {
                            (tab.top_response_number, tab.bottom_response_number)
                        };
                    let was_at_bottom = if i == self.active_response_tab {
                        self.current_response_was_at_bottom
                    } else {
                        tab.was_at_bottom
                    };
                    SessionThreadRef {
                        board_url: tab.board_url.to_string(),
                        dat_file: tab.dat_file.to_string(),
                        title: tab.title.to_string(),
                        top_response_number,
                        bottom_response_number,
                        was_at_bottom,
                    }
                })
                .collect(),
            current_thread: self.current_thread.as_ref().map(|t| SessionThreadRef {
                board_url: t.board_url.to_string(),
                dat_file: t.dat_file.to_string(),
                title: t.title.to_string(),
                top_response_number: self.current_response_top_number,
                bottom_response_number: self.current_response_bottom_number,
                was_at_bottom: self.current_response_was_at_bottom,
            }),
            thread_scroll_y: self.last_thread_scroll_y,
            thread_table_columns: self.cached_thread_table_columns.clone(),
            workspace_sidebar_width: self.workspace_sidebar_width,
            center_split_size: self.center_split_size,
            theme_name: self.current_theme_name.as_ref().map(|s| {
                if let Some(stripped) = s.strip_prefix("Default ") {
                    stripped.to_string()
                } else {
                    s.clone()
                }
            }),
        }
    }

    fn save_session_to_disk(&self) {
        let Ok(path) = Self::session_file_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.build_session_state()) {
            let _ = std::fs::write(path, json);
        }
    }

    fn save_session_to_disk_debounced(&mut self, cx: &mut Context<Self>) {
        const SESSION_SAVE_DEBOUNCE_MILLIS: u64 = 2000;

        self.session_save_debounce_generation += 1;
        let generation = self.session_save_debounce_generation;
        let view = cx.entity();

        cx.spawn(async move |_weak_self, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(
                    SESSION_SAVE_DEBOUNCE_MILLIS,
                ))
                .await;
            _ = view.update(cx, |this, _cx| {
                if this.session_save_debounce_generation == generation {
                    this.save_session_to_disk();
                }
            });
        })
        .detach();
    }

    fn restore_session(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.pending_restore_session.take() else {
            return;
        };

        let fallback_current_thread = session.current_thread.clone();

        self.layout_mode = session.layout_mode;
        self.response_tabs = session
            .response_tabs
            .into_iter()
            .map(|thread| OpenThreadState {
                board_url: thread.board_url.into(),
                dat_file: thread.dat_file.into(),
                title: thread.title.into(),
                top_response_number: thread.top_response_number,
                bottom_response_number: thread.bottom_response_number,
                was_at_bottom: thread.was_at_bottom,
            })
            .collect();
        self.active_response_tab = session
            .active_response_tab
            .min(self.response_tabs.len().saturating_sub(1));
        self.opened_categories = session.opened_categories.into_iter().collect();
        self.pending_restore_thread_scroll_y = Some(session.thread_scroll_y.max(0.0));
        if !session.thread_table_columns.is_empty() {
            self.cached_thread_table_columns = session.thread_table_columns.clone();
            self.pending_restore_thread_table_columns = Some(session.thread_table_columns);
        }
        let active_restore_numbers = self
            .response_tabs
            .get(self.active_response_tab)
            .map(|tab| (tab.top_response_number, tab.bottom_response_number));
        let active_restore_to_bottom = self
            .response_tabs
            .get(self.active_response_tab)
            .map(|tab| tab.was_at_bottom)
            .unwrap_or(false);
        self.pending_restore_response_numbers = active_restore_numbers;
        self.pending_restore_response_scroll_offset_y = None;
        self.pending_restore_response_to_bottom = active_restore_to_bottom;
        self.current_response_top_number = active_restore_numbers.and_then(|(top, _)| top);
        self.current_response_bottom_number = active_restore_numbers.and_then(|(_, bottom)| bottom);
        self.current_response_was_at_bottom = active_restore_to_bottom;
        self.last_thread_scroll_y = session.thread_scroll_y.max(0.0);
        if session.workspace_sidebar_width > 0.0 {
            self.workspace_sidebar_width = session.workspace_sidebar_width;
        }
        if session.center_split_size > 0.0 {
            self.center_split_size = session.center_split_size;
        }
        self.pending_restore_theme = session.theme_name;

        for tab in session.thread_tabs {
            self.open_board_tab(tab.board_url.into(), tab.board_name.into(), cx);
        }
        self.active_thread_tab = session
            .active_thread_tab
            .min(self.thread_tabs.len().saturating_sub(1));

        if let Some(thread) = self
            .response_tabs
            .get(self.active_response_tab)
            .cloned()
            .or_else(|| {
                fallback_current_thread.map(|thread| OpenThreadState {
                    board_url: thread.board_url.into(),
                    dat_file: thread.dat_file.into(),
                    title: thread.title.into(),
                    top_response_number: thread.top_response_number,
                    bottom_response_number: thread.bottom_response_number,
                    was_at_bottom: thread.was_at_bottom,
                })
            })
        {
            self.open_thread(thread.board_url, thread.dat_file, thread.title, cx);
        }

        self.save_session_to_disk_debounced(cx);
        cx.notify();
    }

    fn view(_: &mut Window, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx| Self::new(cx));
        entity.update(cx, |this, cx| {
            this.load_boards(cx);
            this.restore_session(cx);
        });
        entity
    }

    /// バックグラウンドで板一覧を取得し TreeState を更新する
    fn load_boards(&mut self, cx: &mut Context<Self>) {
        let view = cx.entity();
        cx.spawn(async move |_weak_self, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { load_bbsmenu() })
                .await;
            match result {
                Ok(categories) => {
                    _ = view.update(cx, |this, cx| {
                        let mut board_to_category = BTreeMap::new();
                        for cat in &categories {
                            for board in &cat.category_content {
                                board_to_category
                                    .insert(board.url.clone(), cat.category_name.clone());
                            }
                        }
                        this.board_to_category = board_to_category;

                        let items = build_tree_items(&categories, &this.opened_categories);
                        _ = this.tree_state.update(cx, |state, cx| {
                            state.set_items(items, cx);
                        });
                    });
                }
                Err(e) => eprintln!("板一覧の取得に失敗しました: {e}"),
            }
        })
        .detach();
    }

    fn open_board_tab(
        &mut self,
        board_url: SharedString,
        board_name: SharedString,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self
            .thread_tabs
            .iter()
            .position(|tab| tab.board_url == board_url)
        {
            self.active_thread_tab = ix;
            self.save_session_to_disk_debounced(cx);
            cx.notify();
            return;
        }

        if let Some(category) = self.board_to_category.get(board_url.as_ref()) {
            self.opened_categories.insert(category.clone());
        }

        let board_url_for_tab = board_url.clone();
        self.thread_tabs.push(ThreadTab {
            board_name,
            board_url: board_url_for_tab,
            threads: Vec::new(),
            loading: true,
            error: None,
        });
        self.active_thread_tab = self.thread_tabs.len().saturating_sub(1);
        self.save_session_to_disk_debounced(cx);
        cx.notify();

        cx.spawn(async move |weak_self, cx| {
            let board_url_string = board_url.to_string();
            let board_url_for_fetch = board_url_string.clone();
            let result = cx
                .background_executor()
                .spawn(async move { load_subject_lines(&board_url_for_fetch) })
                .await;

            _ = weak_self.update(cx, move |this, cx| {
                if let Some(tab) = this
                    .thread_tabs
                    .iter_mut()
                    .find(|tab| tab.board_url.as_ref() == board_url_string)
                {
                    tab.loading = false;
                    match result {
                        Ok(lines) => {
                            tab.threads = lines;
                            tab.error = None;
                        }
                        Err(e) => {
                            tab.threads.clear();
                            tab.error = Some(e.into());
                        }
                    }
                    this.save_session_to_disk_debounced(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn reload_thread_tab(&mut self, tab_ix: usize, cx: &mut Context<Self>) {
        let Some(tab) = self.thread_tabs.get_mut(tab_ix) else {
            return;
        };
        if tab.loading {
            return;
        }
        tab.loading = true;
        tab.error = None;
        self.footer_status_hint = Some("スレッド一覧を更新中...".into());
        let board_url = tab.board_url.clone();
        self.save_session_to_disk_debounced(cx);
        cx.notify();

        cx.spawn(async move |weak_self, cx| {
            let board_url_s = board_url.to_string();
            let board_for_fetch = board_url_s.clone();
            let result = cx
                .background_executor()
                .spawn(async move { load_subject_lines_force_refresh(&board_for_fetch) })
                .await;
            _ = weak_self.update(cx, move |this, cx| {
                if let Some(tab) = this.thread_tabs.get_mut(tab_ix) {
                    warn!("板タブ '{}' のスレッド一覧を更新", tab.board_name);
                    tab.loading = false;
                    match result {
                        Ok(lines) => {
                            tab.threads = lines;
                            tab.error = None;
                            this.footer_status_hint = None;
                        }
                        Err(e) => {
                            let error_message: SharedString = e.into();
                            tab.error = Some(error_message.clone());
                            this.footer_status_hint =
                                Some(format!("スレッド一覧の更新に失敗: {error_message}").into());
                        }
                    }
                    this.save_session_to_disk_debounced(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn close_thread_tab(&mut self, tab_ix: usize, cx: &mut Context<Self>) {
        if tab_ix >= self.thread_tabs.len() {
            return;
        }
        self.thread_tabs.remove(tab_ix);
        if self.thread_tabs.is_empty() {
            self.active_thread_tab = 0;
        } else {
            self.active_thread_tab = self.active_thread_tab.min(self.thread_tabs.len() - 1);
        }
        self.save_session_to_disk_debounced(cx);
        cx.notify();
    }

    fn reload_current_thread(&mut self, cx: &mut Context<Self>) {
        let Some(current_thread) = self.current_thread.clone() else {
            return;
        };

        // 更新前の表示レス範囲（上端・下端）を保存し、更新後に復元する
        let saved_response_numbers = (
            self.current_response_top_number,
            self.current_response_bottom_number,
        );
        let previous_max_response_number = self
            .responses
            .iter()
            .map(|res| res.number)
            .max()
            .unwrap_or(0);
        let saved_scroll_offset_y = f32::from(self.response_scroll_handle.offset().y);
        if let Some(tab) = self.response_tabs.get_mut(self.active_response_tab) {
            tab.top_response_number = saved_response_numbers.0;
            tab.bottom_response_number = saved_response_numbers.1;
            tab.was_at_bottom = self.current_response_was_at_bottom;
        }

        if !self.has_loaded_response_rows() {
            self.responses = vec![ResponseItem {
                number: 0,
                name: "system".into(),
                timestamp: "".into(),
                id: "".into(),
                body: "レス読み込み中...".into(),
            }];
        }
        self.footer_status_hint = Some("レス一覧を更新中...".into());
        self.save_session_to_disk_debounced(cx);
        cx.notify();

        cx.spawn(async move |weak_self, cx| {
            let board_url_s = current_thread.board_url.to_string();
            let dat_file_s = current_thread.dat_file.to_string();
            let board_for_fetch = board_url_s.clone();
            let dat_for_fetch = dat_file_s.clone();
            let result = cx
                .background_executor()
                .spawn(
                    async move { load_dat_lines_force_refresh(&board_for_fetch, &dat_for_fetch) },
                )
                .await;

            _ = weak_self.update(cx, move |this, cx| {
                match result {
                    Ok(lines) => {
                        warn!("スレ '{}' のレスを更新", current_thread.title);
                        let new_marker_from = lines
                            .iter()
                            .filter(|res| res.number > previous_max_response_number)
                            .map(|res| res.number)
                            .min();
                        this.responses = lines;
                        this.response_new_marker_from = new_marker_from;
                        this.footer_status_hint = None;
                    }
                    Err(e) => {
                        if !this.has_loaded_response_rows() {
                            this.responses = vec![ResponseItem {
                                number: 0,
                                name: "system".into(),
                                timestamp: "".into(),
                                id: "".into(),
                                body: format!("取得失敗: {e}").into(),
                            }];
                        }
                        this.footer_status_hint = Some(format!("レス一覧の更新に失敗: {e}").into());
                    }
                }
                // 更新後は元のピクセルオフセットへ戻し、体感上の移動を防ぐ
                this.pending_restore_response_numbers = None;
                this.pending_restore_response_scroll_offset_y = Some(saved_scroll_offset_y);
                this.pending_restore_response_to_bottom = false;
                this.save_session_to_disk_debounced(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn close_response_tab(&mut self, tab_ix: usize, cx: &mut Context<Self>) {
        if tab_ix >= self.response_tabs.len() {
            return;
        }

        let was_active = tab_ix == self.active_response_tab;
        self.response_tabs.remove(tab_ix);

        if !self.response_tabs.is_empty() {
            if was_active {
                self.active_response_tab = tab_ix.min(self.response_tabs.len().saturating_sub(1));
                if let Some(next_tab) = self.response_tabs.get(self.active_response_tab).cloned() {
                    self.open_thread(next_tab.board_url, next_tab.dat_file, next_tab.title, cx);
                    return;
                }
            } else {
                if tab_ix < self.active_response_tab {
                    self.active_response_tab = self.active_response_tab.saturating_sub(1);
                }
                self.save_session_to_disk_debounced(cx);
                cx.notify();
                return;
            }
        }

        self.active_response_tab = 0;

        self.current_thread = None;
        self.response_composer_open = false;
        self.response_submit_hint = None;
        self.post_confirm_state = None;
        self.current_response_top_number = None;
        self.current_response_bottom_number = None;
        self.current_response_was_at_bottom = false;
        self.response_new_marker_from = None;
        self.pending_restore_response_numbers = None;
        self.pending_restore_response_scroll_offset_y = None;
        self.pending_restore_response_to_bottom = false;
        self.responses = vec![ResponseItem {
            number: 0,
            name: "system".into(),
            timestamp: "".into(),
            id: "".into(),
            body: "スレッドを選択してください".into(),
        }];
        self.save_session_to_disk_debounced(cx);
        cx.notify();
    }

    fn close_current_thread(&mut self, cx: &mut Context<Self>) {
        if self.response_tabs.is_empty() {
            return;
        }
        self.close_response_tab(self.active_response_tab, cx);
    }

    fn gesture_command_label(command: MouseGestureCommand) -> &'static str {
        match command {
            MouseGestureCommand::None => "(未割当)",
            MouseGestureCommand::ScrollTop => "先頭へ",
            MouseGestureCommand::ScrollBottom => "末尾へ",
            MouseGestureCommand::Refresh => "更新",
            MouseGestureCommand::Close => "閉じる",
            MouseGestureCommand::TabPrev => "前のタブ",
            MouseGestureCommand::TabNext => "次のタブ",
        }
    }

    fn schedule_gesture_trail_clear(&mut self, cx: &mut Context<Self>) {
        self.gesture_clear_generation += 1;
        let clear_gen = self.gesture_clear_generation;
        cx.spawn(async move |weak_self, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1500))
                .await;
            _ = weak_self.update(cx, |this, cx| {
                if this.gesture_clear_generation == clear_gen {
                    this.gesture_trail_text = "".into();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn begin_mouse_gesture(
        &mut self,
        pane: GesturePane,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        if !self.mouse_gesture_settings.enabled {
            return;
        }
        // 前回のクリアタイマーをキャンセル
        self.gesture_clear_generation += 1;
        self.gesture_tracking = Some(GestureTrackingState {
            pane,
            last: event.position,
            trail: Vec::new(),
        });
        cx.notify();
    }

    fn detect_mouse_gesture_direction(
        min_distance_px: f32,
        start: Point<Pixels>,
        end: Point<Pixels>,
    ) -> Option<GestureDirection> {
        let min = min_distance_px.max(8.0);
        let dx = f32::from(end.x - start.x);
        let dy = f32::from(end.y - start.y);

        if dx.abs() < min && dy.abs() < min {
            return None;
        }

        if dx.abs() >= dy.abs() {
            if dx >= 0.0 {
                Some(GestureDirection::Right)
            } else {
                Some(GestureDirection::Left)
            }
        } else if dy >= 0.0 {
            Some(GestureDirection::Down)
        } else {
            Some(GestureDirection::Up)
        }
    }

    fn direction_code(direction: GestureDirection) -> char {
        match direction {
            GestureDirection::Up => 'u',
            GestureDirection::Down => 'd',
            GestureDirection::Left => 'l',
            GestureDirection::Right => 'r',
        }
    }

    fn direction_label(direction: GestureDirection) -> &'static str {
        match direction {
            GestureDirection::Up => "↑",
            GestureDirection::Down => "↓",
            GestureDirection::Left => "←",
            GestureDirection::Right => "→",
        }
    }

    fn gesture_sequence_key(trail: &[GestureDirection]) -> String {
        trail
            .iter()
            .map(|direction| Self::direction_code(*direction))
            .collect()
    }

    fn gesture_sequence_label(trail: &[GestureDirection]) -> String {
        trail
            .iter()
            .map(|direction| Self::direction_label(*direction))
            .collect::<Vec<_>>()
            .join("")
    }

    fn track_mouse_gesture_move(
        &mut self,
        pane: GesturePane,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        let min_distance_px = self.mouse_gesture_settings.min_distance_px;

        // 借用競合を避けるためブロック内で必要データを抽出する
        let result = {
            let Some(tracking) = self.gesture_tracking.as_mut() else {
                return;
            };
            if tracking.pane != pane {
                return;
            }
            let Some(direction) = Self::detect_mouse_gesture_direction(
                min_distance_px,
                tracking.last,
                event.position,
            ) else {
                return;
            };
            let changed = tracking.trail.last().copied() != Some(direction);
            if changed {
                tracking.trail.push(direction);
            }
            tracking.last = event.position;
            if !changed {
                return;
            }
            (
                Self::gesture_sequence_key(&tracking.trail),
                Self::gesture_sequence_label(&tracking.trail),
                tracking.pane,
            )
        };

        let (trail_key, trail_label, track_pane) = result;
        let bindings = match track_pane {
            GesturePane::ThreadList => &self.mouse_gesture_settings.thread,
            GesturePane::ResponseList => &self.mouse_gesture_settings.response,
        };
        let command = Self::command_for_gesture(bindings, &trail_key);
        let cmd_label = Self::gesture_command_label(command);
        self.gesture_trail_text = format!("ジェスチャ: {trail_label} {cmd_label}").into();
        cx.notify();
    }

    fn command_for_gesture(
        bindings: &MouseGesturePaneBindings,
        gesture_key: &str,
    ) -> MouseGestureCommand {
        bindings
            .commands
            .get(gesture_key)
            .copied()
            .unwrap_or(MouseGestureCommand::None)
    }

    fn execute_gesture_command(
        &mut self,
        pane: GesturePane,
        command: MouseGestureCommand,
        cx: &mut Context<Self>,
    ) {
        match (pane, command) {
            (_, MouseGestureCommand::None) => {}
            (GesturePane::ThreadList, MouseGestureCommand::ScrollTop) => {
                if let Some(state) = &self.thread_table_state {
                    state.update(cx, |state, _cx| {
                        state
                            .vertical_scroll_handle
                            .scroll_to_item(0, ScrollStrategy::Top);
                    });
                    cx.notify();
                }
            }
            (GesturePane::ThreadList, MouseGestureCommand::ScrollBottom) => {
                let selected_tab_ix = if self.thread_tabs.is_empty() {
                    0
                } else {
                    self.active_thread_tab
                        .min(self.thread_tabs.len().saturating_sub(1))
                };
                let count = self
                    .thread_tabs
                    .get(selected_tab_ix)
                    .map(|tab| tab.threads.len())
                    .unwrap_or(0);
                if let Some(state) = &self.thread_table_state {
                    state.update(cx, |state, _cx| {
                        if count > 0 {
                            state
                                .vertical_scroll_handle
                                .scroll_to_item(count - 1, ScrollStrategy::Top);
                        }
                    });
                    cx.notify();
                }
            }
            (GesturePane::ThreadList, MouseGestureCommand::Refresh) => {
                self.reset_pull_refresh_state();
                if !self.thread_tabs.is_empty() {
                    let ix = self.active_thread_tab;
                    self.reload_thread_tab(ix, cx);
                }
            }
            (GesturePane::ThreadList, MouseGestureCommand::Close) => {
                if !self.thread_tabs.is_empty() {
                    let ix = self.active_thread_tab;
                    self.close_thread_tab(ix, cx);
                }
            }
            (GesturePane::ResponseList, MouseGestureCommand::ScrollTop) => {
                self.response_scroll_handle
                    .scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            }
            (GesturePane::ResponseList, MouseGestureCommand::ScrollBottom) => {
                let count = self.responses.len();
                if count > 0 {
                    self.response_scroll_handle
                        .scroll_to_item(count - 1, ScrollStrategy::Top);
                }
                cx.notify();
            }
            (GesturePane::ResponseList, MouseGestureCommand::Refresh) => {
                self.reset_pull_refresh_state();
                self.reload_current_thread(cx);
            }
            (GesturePane::ResponseList, MouseGestureCommand::Close) => {
                self.close_current_thread(cx);
            }
            (GesturePane::ThreadList, MouseGestureCommand::TabPrev) => {
                if !self.thread_tabs.is_empty() && self.active_thread_tab > 0 {
                    self.active_thread_tab -= 1;
                    self.save_session_to_disk_debounced(cx);
                    cx.notify();
                }
            }
            (GesturePane::ThreadList, MouseGestureCommand::TabNext) => {
                if !self.thread_tabs.is_empty()
                    && self.active_thread_tab < self.thread_tabs.len() - 1
                {
                    self.active_thread_tab += 1;
                    self.save_session_to_disk_debounced(cx);
                    cx.notify();
                }
            }
            (GesturePane::ResponseList, MouseGestureCommand::TabPrev) => {
                if !self.response_tabs.is_empty() && self.active_response_tab > 0 {
                    let new_ix = self.active_response_tab - 1;
                    let tab = self.response_tabs[new_ix].clone();
                    self.active_response_tab = new_ix;
                    self.open_thread(tab.board_url, tab.dat_file, tab.title, cx);
                }
            }
            (GesturePane::ResponseList, MouseGestureCommand::TabNext) => {
                if !self.response_tabs.is_empty()
                    && self.active_response_tab < self.response_tabs.len() - 1
                {
                    let new_ix = self.active_response_tab + 1;
                    let tab = self.response_tabs[new_ix].clone();
                    self.active_response_tab = new_ix;
                    self.open_thread(tab.board_url, tab.dat_file, tab.title, cx);
                }
            }
        }
    }

    fn set_focused_pane(&mut self, pane: GesturePane) {
        self.focused_pane = pane;
    }

    fn command_for_key(
        bindings: &KeyBindingPaneBindings,
        control: bool,
        key: &str,
    ) -> Option<KeyBindingCommand> {
        if control {
            return match key {
                "r" => Some(bindings.ctrl_r),
                "w" => Some(bindings.ctrl_w),
                _ => None,
            };
        }

        match key {
            "pageup" => Some(bindings.page_up),
            "pagedown" => Some(bindings.page_down),
            "home" => Some(bindings.home),
            "end" => Some(bindings.end),
            "f5" => Some(bindings.f5),
            _ => None,
        }
    }

    fn scroll_thread_by_px(&self, delta_y: f32, cx: &mut Context<Self>) {
        if let Some(state) = &self.thread_table_state {
            state.update(cx, |state, _cx| {
                let current_y = f32::from(state.vertical_scroll_handle.offset().y);
                let next_y = (current_y - delta_y).min(0.0);
                state
                    .vertical_scroll_handle
                    .set_offset(point(px(0.), px(next_y)));
            });
            cx.notify();
        }
    }

    fn scroll_response_by_px(&self, delta_y: f32) {
        let current_y = f32::from(self.response_scroll_handle.offset().y);
        let next_y = (current_y - delta_y).min(0.0);
        self.response_scroll_handle
            .set_offset(point(px(0.), px(next_y)));
    }

    fn reset_pull_refresh_state(&mut self) {
        self.pull_refresh_hint = None;
        self.thread_pull_refresh_accum_px = 0.0;
        self.response_pull_refresh_accum_px = 0.0;
    }

    fn scroll_wheel_delta_y_px(event: &ScrollWheelEvent) -> f32 {
        let line_height = px(22.);
        let pixel_delta = match event.delta {
            ScrollDelta::Pixels(delta) => delta,
            ScrollDelta::Lines(_) => event.delta.pixel_delta(line_height),
        };
        f32::from(pixel_delta.y)
    }

    fn response_bottom_offset_px(&self) -> f32 {
        (self.response_viewport_height_px - self.response_content_height_px).min(0.0)
    }

    fn handle_pull_refresh_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        const PULL_REFRESH_THRESHOLD_PX: f32 = 360.0;

        if self.modal_state.is_some() {
            return;
        }

        let delta_y = Self::scroll_wheel_delta_y_px(event);
        if delta_y.abs() < 0.01 {
            return;
        }

        match self.focused_pane {
            GesturePane::ThreadList => {
                self.response_pull_refresh_accum_px = 0.0;
                let at_top = self.current_thread_scroll_y(cx) >= -1.0;
                let pulling_for_refresh = delta_y > 0.0;

                if at_top && pulling_for_refresh {
                    self.thread_pull_refresh_accum_px += delta_y.abs();
                    let remain =
                        (PULL_REFRESH_THRESHOLD_PX - self.thread_pull_refresh_accum_px).max(0.0);
                    self.pull_refresh_hint =
                        Some(format!("スレッド一覧更新まであと {:.0}px", remain).into());

                    if remain <= 0.0 {
                        self.thread_pull_refresh_accum_px = 0.0;
                        self.pull_refresh_hint = Some("スレッド一覧を更新中...".into());
                        self.execute_gesture_command(
                            GesturePane::ThreadList,
                            MouseGestureCommand::Refresh,
                            cx,
                        );
                    }
                } else {
                    self.thread_pull_refresh_accum_px = 0.0;
                    self.pull_refresh_hint = None;
                }
            }
            GesturePane::ResponseList => {
                self.thread_pull_refresh_accum_px = 0.0;
                let bottom_offset = self.response_bottom_offset_px();
                let current_offset = f32::from(self.response_scroll_handle.offset().y);
                let at_bottom = current_offset <= bottom_offset + 1.0;
                let pulling_for_refresh = delta_y < 0.0;

                if at_bottom && pulling_for_refresh {
                    self.response_pull_refresh_accum_px += delta_y.abs();
                    let remain =
                        (PULL_REFRESH_THRESHOLD_PX - self.response_pull_refresh_accum_px).max(0.0);
                    self.pull_refresh_hint =
                        Some(format!("レス更新まであと {:.0}px", remain).into());

                    if remain <= 0.0 {
                        self.response_pull_refresh_accum_px = 0.0;
                        self.pull_refresh_hint = Some("レス一覧を更新中...".into());
                        self.execute_gesture_command(
                            GesturePane::ResponseList,
                            MouseGestureCommand::Refresh,
                            cx,
                        );
                    }
                } else {
                    self.response_pull_refresh_accum_px = 0.0;
                    self.pull_refresh_hint = None;
                }
            }
        }

        cx.notify();
    }

    fn execute_key_binding_command(
        &mut self,
        pane: GesturePane,
        command: KeyBindingCommand,
        cx: &mut Context<Self>,
    ) {
        const PAGE_SCROLL_DELTA_PX: f32 = 480.0;

        match (pane, command) {
            (_, KeyBindingCommand::None) => {}
            (pane, KeyBindingCommand::ScrollTop) => {
                self.execute_gesture_command(pane, MouseGestureCommand::ScrollTop, cx);
            }
            (pane, KeyBindingCommand::ScrollBottom) => {
                self.execute_gesture_command(pane, MouseGestureCommand::ScrollBottom, cx);
            }
            (pane, KeyBindingCommand::Refresh) => {
                self.execute_gesture_command(pane, MouseGestureCommand::Refresh, cx);
            }
            (pane, KeyBindingCommand::Close) => {
                self.execute_gesture_command(pane, MouseGestureCommand::Close, cx);
            }
            (GesturePane::ThreadList, KeyBindingCommand::PageUp) => {
                self.scroll_thread_by_px(-PAGE_SCROLL_DELTA_PX, cx);
            }
            (GesturePane::ThreadList, KeyBindingCommand::PageDown) => {
                self.scroll_thread_by_px(PAGE_SCROLL_DELTA_PX, cx);
            }
            (GesturePane::ResponseList, KeyBindingCommand::PageUp) => {
                self.scroll_response_by_px(-PAGE_SCROLL_DELTA_PX);
                cx.notify();
            }
            (GesturePane::ResponseList, KeyBindingCommand::PageDown) => {
                self.scroll_response_by_px(PAGE_SCROLL_DELTA_PX);
                cx.notify();
            }
        }
    }

    fn handle_key_binding_key(&mut self, key: &str, control: bool, cx: &mut Context<Self>) {
        if !self.key_binding_settings.enabled {
            return;
        }
        if self.modal_state.is_some() {
            return;
        }

        let key = key.to_ascii_lowercase();
        let bindings = match self.focused_pane {
            GesturePane::ThreadList => &self.key_binding_settings.thread,
            GesturePane::ResponseList => &self.key_binding_settings.response,
        };
        let Some(command) = Self::command_for_key(bindings, control, key.as_str()) else {
            return;
        };
        self.execute_key_binding_command(self.focused_pane, command, cx);
    }

    fn end_mouse_gesture(
        &mut self,
        pane: GesturePane,
        event: &MouseUpEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let min_distance_px = self.mouse_gesture_settings.min_distance_px;
        // マウスアップ時の最終移動方向も軌跡に追加する
        if let Some(tracking) = self.gesture_tracking.as_mut() {
            if tracking.pane == pane {
                if let Some(direction) = Self::detect_mouse_gesture_direction(
                    min_distance_px,
                    tracking.last,
                    event.position,
                ) {
                    if tracking.trail.last().copied() != Some(direction) {
                        tracking.trail.push(direction);
                    }
                }
                tracking.last = event.position;
            }
        }

        let Some(tracking) = self.gesture_tracking.take() else {
            return false;
        };
        if tracking.pane != pane {
            return false;
        }

        if tracking.trail.is_empty() {
            // 軌跡なし: 短時間後にクリア
            self.schedule_gesture_trail_clear(cx);
            return false;
        }

        let gesture_key = Self::gesture_sequence_key(&tracking.trail);
        let trail_label = Self::gesture_sequence_label(&tracking.trail);

        let bindings = match pane {
            GesturePane::ThreadList => &self.mouse_gesture_settings.thread,
            GesturePane::ResponseList => &self.mouse_gesture_settings.response,
        };
        let command = Self::command_for_gesture(bindings, gesture_key.as_str());
        let cmd_label = Self::gesture_command_label(command);
        self.gesture_trail_text = format!("ジェスチャ: {trail_label} {cmd_label}").into();
        cx.notify();

        self.execute_gesture_command(pane, command, cx);
        // 実行/未割当どちらも一定時間後に軌跡を消す
        self.schedule_gesture_trail_clear(cx);
        true
    }

    fn open_thread_pane_context_menu(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity();
        let menu = PopupMenu::build(window, cx, move |menu, _window, _cx| {
            let view1 = view.clone();
            let view2 = view.clone();
            menu.item(PopupMenuItem::new("更新").on_click(move |_, _, cx| {
                _ = view1.update(cx, |this, cx| {
                    if !this.thread_tabs.is_empty() {
                        let ix = this.active_thread_tab;
                        this.reload_thread_tab(ix, cx);
                    }
                });
            }))
            .separator()
            .item(PopupMenuItem::new("閉じる").on_click(move |_, _, cx| {
                _ = view2.update(cx, |this, cx| {
                    if !this.thread_tabs.is_empty() {
                        let ix = this.active_thread_tab;
                        this.close_thread_tab(ix, cx);
                    }
                });
            }))
        });

        let subscription = cx.subscribe(
            &menu,
            |this: &mut Self,
             _menu: Entity<PopupMenu>,
             _: &DismissEvent,
             cx: &mut Context<Self>| {
                this.thread_pane_context_menu.open = false;
                this.thread_pane_context_menu.menu_view = None;
                this.thread_pane_context_menu.dismiss_subscription = None;
                cx.notify();
            },
        );

        self.thread_pane_context_menu.position = position;
        self.thread_pane_context_menu.menu_view = Some(menu);
        self.thread_pane_context_menu.dismiss_subscription = Some(subscription);
        self.thread_pane_context_menu.open = true;
        window.refresh();
    }

    fn open_response_row_context_menu(
        &mut self,
        position: Point<Pixels>,
        body: String,
        name: String,
        id: String,
        full: String,
        urls: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = PopupMenu::build(window, cx, move |menu, _window, _cx| {
            let mut menu = menu
                .item(PopupMenuItem::new("本文をコピー").on_click({
                    let body = body.clone();
                    move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(body.clone()));
                    }
                }))
                .item(PopupMenuItem::new("名前をコピー").on_click({
                    let name = name.clone();
                    move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(name.clone()));
                    }
                }))
                .item(PopupMenuItem::new("IDをコピー").on_click({
                    let id = id.clone();
                    move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(id.clone()));
                    }
                }))
                .separator()
                .item(PopupMenuItem::new("全文をコピー").on_click({
                    let full = full.clone();
                    move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(full.clone()));
                    }
                }));

            if !urls.is_empty() {
                menu = menu.separator().item(PopupMenuItem::label("URL"));
                for url in urls {
                    let open_url = url.clone();
                    let copy_url = url.clone();
                    menu = menu
                        .item(
                            PopupMenuItem::new(format!("外部ブラウザで開く: {url}")).on_click(
                                move |_, _, _cx| {
                                    Self::open_url_in_external_browser(&open_url);
                                },
                            ),
                        )
                        .item(PopupMenuItem::new(format!("URLをコピー: {url}")).on_click(
                            move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy_url.clone()));
                            },
                        ));
                }
            }

            menu
        });

        let subscription = cx.subscribe(
            &menu,
            |this: &mut Self,
             _menu: Entity<PopupMenu>,
             _: &DismissEvent,
             cx: &mut Context<Self>| {
                this.response_row_context_menu.open = false;
                this.response_row_context_menu.menu_view = None;
                this.response_row_context_menu.dismiss_subscription = None;
                cx.notify();
            },
        );

        self.response_row_context_menu.position = position;
        self.response_row_context_menu.menu_view = Some(menu);
        self.response_row_context_menu.dismiss_subscription = Some(subscription);
        self.response_row_context_menu.open = true;
        window.refresh();
    }

    fn ensure_response_composer_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.response_form_name_state.is_none() {
            self.response_form_name_state =
                Some(cx.new(|cx| InputState::new(window, cx).placeholder("名前")));
        }
        if self.response_form_mail_state.is_none() {
            self.response_form_mail_state =
                Some(cx.new(|cx| InputState::new(window, cx).placeholder("mail")));
        }
        if self.response_form_body_state.is_none() {
            self.response_form_body_state = Some(cx.new(|cx| {
                InputState::new(window, cx)
                    .multi_line(true)
                    .rows(6)
                    .placeholder("message")
            }));
        }
    }

    fn toggle_response_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.response_composer_open = !self.response_composer_open;
        if self.response_composer_open {
            self.ensure_response_composer_inputs(window, cx);
        }
        cx.notify();
    }

    fn submit_response_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_response_composer_inputs(window, cx);

        let Some(name_state) = self.response_form_name_state.as_ref() else {
            return;
        };
        let Some(mail_state) = self.response_form_mail_state.as_ref() else {
            return;
        };
        let Some(body_state) = self.response_form_body_state.as_ref() else {
            return;
        };

        let _name = name_state.read(cx).value();
        let _mail = mail_state.read(cx).value();
        let body = body_state.read(cx).value();
        let Some(_thread) = self.current_thread.clone() else {
            self.show_alert("エラー", "投稿対象のスレッドが未選択です", cx);
            return;
        };

        if body.trim().is_empty() {
            self.show_confirmation(
                "確認",
                "本文が空ですが投稿しますか？",
                "投稿する",
                "キャンセル",
                cx,
            );
            return;
        }

        // 本文がある場合は投稿を実行
        self.execute_post_submission(window, cx);
    }

    fn open_thread(
        &mut self,
        board_url: SharedString,
        dat_file: SharedString,
        title: SharedString,
        cx: &mut Context<Self>,
    ) {
        // 現在のアクティブタブの表示レス範囲を保存してから切り替える
        let current_response_numbers = (
            self.current_response_top_number,
            self.current_response_bottom_number,
        );
        if let Some(cur_tab) = self.response_tabs.get_mut(self.active_response_tab) {
            cur_tab.top_response_number = current_response_numbers.0;
            cur_tab.bottom_response_number = current_response_numbers.1;
            cur_tab.was_at_bottom = self.current_response_was_at_bottom;
        }

        let tab_ix = self
            .response_tabs
            .iter()
            .position(|tab| tab.board_url == board_url && tab.dat_file == dat_file);
        let restore_response_numbers;
        let restore_to_bottom;
        if let Some(ix) = tab_ix {
            // 既存タブへの切り替え：保存済み表示レス範囲を復元する
            restore_response_numbers = (
                self.response_tabs[ix].top_response_number,
                self.response_tabs[ix].bottom_response_number,
            );
            restore_to_bottom = self.response_tabs[ix].was_at_bottom;
            self.active_response_tab = ix;
            self.response_tabs[ix].title = title.clone();
        } else {
            restore_response_numbers = (None, None);
            restore_to_bottom = false;
            self.response_tabs.push(OpenThreadState {
                board_url: board_url.clone(),
                dat_file: dat_file.clone(),
                title: title.clone(),
                top_response_number: None,
                bottom_response_number: None,
                was_at_bottom: false,
            });
            self.active_response_tab = self.response_tabs.len().saturating_sub(1);
        }

        self.post_confirm_state = None;
        self.response_submit_hint = None;
        self.current_thread = Some(OpenThreadState {
            board_url: board_url.clone(),
            dat_file: dat_file.clone(),
            title: title.clone(),
            top_response_number: restore_response_numbers.0,
            bottom_response_number: restore_response_numbers.1,
            was_at_bottom: restore_to_bottom,
        });
        self.current_response_top_number = restore_response_numbers.0;
        self.current_response_bottom_number = restore_response_numbers.1;
        self.current_response_was_at_bottom = restore_to_bottom;
        self.response_new_marker_from = None;
        self.responses = vec![ResponseItem {
            number: 0,
            name: "system".into(),
            timestamp: "".into(),
            id: "".into(),
            body: "レス読み込み中...".into(),
        }];
        // ロード完了後にレス番号から位置を復元するようにセット
        self.pending_restore_response_numbers = Some(restore_response_numbers);
        self.pending_restore_response_scroll_offset_y = None;
        self.pending_restore_response_to_bottom = restore_to_bottom;
        self.save_session_to_disk_debounced(cx);
        cx.notify();

        cx.spawn(async move |weak_self, cx| {
            let board_url_s = board_url.to_string();
            let dat_file_s = dat_file.to_string();
            let board_for_fetch = board_url_s.clone();
            let dat_for_fetch = dat_file_s.clone();
            let result = cx
                .background_executor()
                .spawn(async move { load_dat_lines(&board_for_fetch, &dat_for_fetch) })
                .await;

            _ = weak_self.update(cx, move |this, cx| {
                match result {
                    Ok(lines) => this.responses = lines,
                    Err(e) => {
                        this.responses = vec![ResponseItem {
                            number: 0,
                            name: "system".into(),
                            timestamp: "".into(),
                            id: "".into(),
                            body: format!("取得失敗: {e}").into(),
                        }]
                    }
                }
                // ロード後に保存済みレス番号を再セット（レンダリングで適用される）
                this.pending_restore_response_numbers = Some(restore_response_numbers);
                this.pending_restore_response_scroll_offset_y = None;
                this.pending_restore_response_to_bottom = restore_to_bottom;
                this.save_session_to_disk_debounced(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn response_tab_label(title: &str) -> SharedString {
        const MAX_WIDTH: usize = 20;
        const ELLIPSIS: &str = "…";

        if UnicodeWidthStr::width(title) <= MAX_WIDTH {
            return title.to_string().into();
        }

        let mut out = String::new();
        let mut width = 0usize;
        let limit = MAX_WIDTH.saturating_sub(ELLIPSIS.len());

        for ch in title.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if width + ch_width > limit {
                break;
            }
            out.push(ch);
            width += ch_width;
        }

        out.push_str(ELLIPSIS);
        out.into()
    }

    /// 確認ダイアログを表示する
    fn show_confirmation(
        &mut self,
        title: impl Into<SharedString>,
        message: impl Into<SharedString>,
        ok_label: impl Into<SharedString>,
        cancel_label: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.modal_state = Some(Modal::Confirmation {
            title: title.into(),
            message: message.into(),
            ok_label: ok_label.into(),
            cancel_label: cancel_label.into(),
        });
        cx.notify();
    }

    /// アラート（確認なし）を表示する
    fn show_alert(
        &mut self,
        title: impl Into<SharedString>,
        message: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.modal_state = Some(Modal::Alert {
            title: title.into(),
            message: message.into(),
        });
        cx.notify();
    }

    /// モーダルを閉じる
    fn close_modal(&mut self, cx: &mut Context<Self>) {
        self.modal_state = None;
        cx.notify();
    }

    /// 実際の投稿処理を実行する
    fn execute_post_submission(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_response_composer_inputs(window, cx);

        let Some(name_state) = self.response_form_name_state.as_ref() else {
            return;
        };
        let Some(mail_state) = self.response_form_mail_state.as_ref() else {
            return;
        };
        let Some(body_state) = self.response_form_body_state.as_ref() else {
            return;
        };

        let name = name_state.read(cx).value();
        let mail = mail_state.read(cx).value();
        let body = body_state.read(cx).value();
        let use_sage = self.response_form_sage;
        let Some(thread) = self.current_thread.clone() else {
            self.show_alert("エラー", "投稿対象のスレッドが未選択です", cx);
            return;
        };

        let pending_confirm = self.post_confirm_state.as_ref().and_then(|state| {
            if state.board_url == thread.board_url && state.dat_file == thread.dat_file {
                Some((state.feature.to_string(), state.time.to_string()))
            } else {
                None
            }
        });

        self.response_submit_hint = Some("投稿処理を送信中...".into());
        cx.notify();

        let board_url = thread.board_url.to_string();
        let dat_file = thread.dat_file.to_string();
        let board_url_for_state = board_url.clone();
        let dat_file_for_state = dat_file.clone();
        let name_s = name.to_string();
        let mail_s = mail.to_string();
        let body_s = body.to_string();

        cx.spawn(async move |weak_self, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    post_5ch(
                        &board_url,
                        &dat_file,
                        &name_s,
                        &mail_s,
                        &body_s,
                        use_sage,
                        pending_confirm,
                    )
                })
                .await;

            _ = weak_self.update(cx, move |this, cx| {
                match result {
                    Ok(PostAttemptResult::Posted) => {
                        this.post_confirm_state = None;
                        this.response_submit_hint = Some("書き込みが完了しました".into());
                    }
                    Ok(PostAttemptResult::NeedConfirm { feature, time }) => {
                        this.post_confirm_state = Some(PostConfirmState {
                            board_url: board_url_for_state.clone().into(),
                            dat_file: dat_file_for_state.clone().into(),
                            feature: feature.into(),
                            time: time.into(),
                        });
                        this.response_submit_hint = Some(
                            "確認ページを通過しました。もう一度「書き込む」で投稿が完了します"
                                .into(),
                        );
                    }
                    Err(err) => {
                        this.response_submit_hint = Some(format!("投稿失敗: {err}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn new(cx: &mut App) -> Self {
        let tree_state = cx.new(|cx| TreeState::new(cx));
        let thread_tabs = Vec::new();
        let response_tabs = Vec::new();
        let session = Self::load_session_from_disk();
        let mouse_gesture_settings = Self::load_mouse_gesture_settings_from_disk();
        let key_binding_settings = Self::load_key_binding_settings_from_disk();

        let responses = (1..=5)
            .map(|i| ResponseItem {
                number: i,
                name: "既にその名前は使われています".into(),
                timestamp: format!("2026/04/19(日) 09:{:02}:00", i % 60).into(),
                id: format!("ID:{:02X}{:02X}", i, 255 - i).into(),
                body: "本文ダミー\n2行目の本文ダミー".into(),
            })
            .collect();

        let this = Self {
            layout_mode: LayoutMode::Horizontal,
            active_thread_tab: 0,
            active_response_tab: 0,
            thread_scroll_handle: VirtualListScrollHandle::new(),
            thread_table_state: None,
            thread_table_subscription: None,
            thread_table_source_signature: 0,
            response_scroll_handle: VirtualListScrollHandle::new(),
            response_virtual_body_width_px: 640.0,
            tree_state,
            thread_tabs,
            response_tabs,
            responses,
            board_to_category: BTreeMap::new(),
            opened_categories: BTreeSet::new(),
            current_thread: None,
            pending_restore_thread_scroll_y: None,
            pending_restore_thread_table_columns: None,
            pending_restore_response_numbers: None,
            pending_restore_response_scroll_offset_y: None,
            pending_restore_response_to_bottom: false,
            pending_restore_session: session,
            last_thread_scroll_y: 0.0,
            current_response_top_number: None,
            current_response_bottom_number: None,
            current_response_was_at_bottom: false,
            response_new_marker_from: None,
            workspace_sidebar_width: 250.0,
            center_split_size: 360.0,
            pending_restore_theme: None,
            current_theme_name: None,
            response_composer_open: false,
            response_form_name_state: None,
            response_form_mail_state: None,
            response_form_body_state: None,
            response_form_sage: false,
            response_submit_hint: None,
            post_confirm_state: None,
            modal_state: None,
            post_pending_confirm_empty_body: false,
            mouse_gesture_settings,
            key_binding_settings,
            gesture_tracking: None,
            gesture_trail_text: "".into(),
            gesture_clear_generation: 0,
            focused_pane: GesturePane::ThreadList,
            thread_pane_context_menu: PaneContextMenuState::default(),
            response_row_context_menu: PaneContextMenuState::default(),
            footer_status_hint: None,
            pull_refresh_hint: None,
            thread_pull_refresh_accum_px: 0.0,
            response_pull_refresh_accum_px: 0.0,
            response_content_height_px: 0.0,
            response_viewport_height_px: 0.0,
            image_preview_state: None,
            image_preview_generation: 0,
            cached_thread_table_columns: Vec::new(),
            session_save_debounce_generation: 0,
        };
        this.save_mouse_gesture_settings_to_disk();
        this.save_key_binding_settings_to_disk();
        this
    }

    fn current_thread_source_signature(&self) -> u64 {
        let mut hasher = DefaultHasher::new();

        let selected_tab_ix = if self.thread_tabs.is_empty() {
            0
        } else {
            self.active_thread_tab
                .min(self.thread_tabs.len().saturating_sub(1))
        };
        selected_tab_ix.hash(&mut hasher);

        if let Some(tab) = self.thread_tabs.get(selected_tab_ix) {
            tab.board_url.as_ref().hash(&mut hasher);
            tab.loading.hash(&mut hasher);
            tab.error.as_ref().map(|e| e.as_ref()).hash(&mut hasher);
            for t in &tab.threads {
                t.dat_file.as_ref().hash(&mut hasher);
                t.title.as_ref().hash(&mut hasher);
                t.response_count.hash(&mut hasher);
            }
        } else {
            "no-tab".hash(&mut hasher);
        }

        hasher.finish()
    }

    fn current_thread_scroll_y(&self, cx: &App) -> f32 {
        if let Some(state) = &self.thread_table_state {
            return f32::from(state.read(cx).vertical_scroll_handle.offset().y);
        }
        f32::from(self.thread_scroll_handle.offset().y)
    }

    fn current_thread_rows(&self) -> (Vec<ThreadTableRow>, SharedString) {
        let selected_tab_ix = if self.thread_tabs.is_empty() {
            0
        } else {
            self.active_thread_tab
                .min(self.thread_tabs.len().saturating_sub(1))
        };

        let active_tab = self.thread_tabs.get(selected_tab_ix);
        if let Some(tab) = active_tab {
            if tab.loading && tab.threads.is_empty() {
                (Vec::new(), "読み込み中...".into())
            } else if let Some(err) = &tab.error {
                if tab.threads.is_empty() {
                    (Vec::new(), format!("取得失敗: {err}").into())
                } else {
                    (
                        tab.threads
                            .iter()
                            .enumerate()
                            .map(|(ix, thread)| ThreadTableRow {
                                number: ix + 1,
                                board_url: tab.board_url.clone(),
                                dat_file: thread.dat_file.clone(),
                                title: thread.title.clone(),
                                response_count: thread.response_count,
                                created_at: format_thread_created_at(&thread.dat_file),
                                momentum_per_day: format_thread_momentum_per_day(
                                    thread.response_count,
                                    &thread.dat_file,
                                ),
                            })
                            .collect(),
                        "".into(),
                    )
                }
            } else if tab.threads.is_empty() {
                (Vec::new(), "スレッドがありません".into())
            } else {
                (
                    tab.threads
                        .iter()
                        .enumerate()
                        .map(|(ix, thread)| ThreadTableRow {
                            number: ix + 1,
                            board_url: tab.board_url.clone(),
                            dat_file: thread.dat_file.clone(),
                            title: thread.title.clone(),
                            response_count: thread.response_count,
                            created_at: format_thread_created_at(&thread.dat_file),
                            momentum_per_day: format_thread_momentum_per_day(
                                thread.response_count,
                                &thread.dat_file,
                            ),
                        })
                        .collect(),
                    "".into(),
                )
            }
        } else {
            (Vec::new(), "左の板一覧から板を選択してください".into())
        }
    }

    fn current_board_title(&self) -> SharedString {
        if self.thread_tabs.is_empty() {
            return "スレッド一覧".into();
        }

        let selected_tab_ix = self
            .active_thread_tab
            .min(self.thread_tabs.len().saturating_sub(1));
        self.thread_tabs
            .get(selected_tab_ix)
            .map(|tab| tab.board_name.clone())
            .unwrap_or_else(|| "スレッド一覧".into())
    }

    fn current_response_title(&self) -> SharedString {
        self.current_thread
            .as_ref()
            .map(|thread| thread.title.clone())
            .unwrap_or_else(|| "レス表示".into())
    }

    fn ensure_thread_table_state(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TableState<ThreadTableDelegate>> {
        if let Some(state) = &self.thread_table_state {
            return state.clone();
        }

        let state = cx.new(|cx| TableState::new(ThreadTableDelegate::new(), window, cx));
        let subscription = cx.subscribe(
            &state,
            |this: &mut Self, table_state, ev: &TableEvent, cx| match ev {
                TableEvent::SelectRow(row_ix) => {
                    let picked = table_state.read(cx).delegate().row(*row_ix).cloned();
                    if let Some(row) = picked {
                        this.open_thread(
                            row.board_url.clone(),
                            row.dat_file.clone(),
                            row.title.clone(),
                            cx,
                        );
                    }
                }
                TableEvent::ColumnWidthsChanged(widths) => {
                    this.cached_thread_table_columns = table_state
                        .read(cx)
                        .delegate()
                        .columns
                        .iter()
                        .enumerate()
                        .map(|(ix, c)| SessionTableColumn {
                            key: c.key.to_string(),
                            width_px: widths
                                .get(ix)
                                .map(|w| f32::from(*w))
                                .unwrap_or_else(|| f32::from(c.width)),
                        })
                        .collect();
                    this.save_session_to_disk_debounced(cx);
                }
                TableEvent::MoveColumn(_, _) => {
                    let old_widths: std::collections::HashMap<String, f32> = this
                        .cached_thread_table_columns
                        .iter()
                        .map(|c| (c.key.clone(), c.width_px))
                        .collect();

                    this.cached_thread_table_columns = table_state
                        .read(cx)
                        .delegate()
                        .columns
                        .iter()
                        .map(|c| SessionTableColumn {
                            key: c.key.to_string(),
                            width_px: old_widths
                                .get(c.key.as_ref())
                                .copied()
                                .unwrap_or_else(|| f32::from(c.width)),
                        })
                        .collect();
                    this.save_session_to_disk_debounced(cx);
                }
                _ => {}
            },
        );

        self.thread_table_state = Some(state.clone());
        self.thread_table_subscription = Some(subscription);
        state
    }

    fn set_layout_mode(&mut self, mode: LayoutMode, cx: &mut Context<Self>) {
        self.layout_mode = mode;
        self.save_session_to_disk_debounced(cx);
        cx.notify();
    }

    fn panel_shell(
        title: impl Into<SharedString>,
        body: impl IntoElement,
        bg: gpui::Hsla,
        border: gpui::Hsla,
        title_bg: gpui::Hsla,
        title_fg: gpui::Hsla,
    ) -> AnyElement {
        let title: SharedString = title.into();

        v_flex()
            .size_full()
            .bg(bg)
            .border_1()
            .border_color(border)
            .child(
                h_flex()
                    .h(px(28.))
                    .items_center()
                    .px_3()
                    .bg(title_bg)
                    .border_b_1()
                    .border_color(border)
                    .text_sm()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(title_fg)
                    .child(title),
            )
            .child(div().flex_1().min_h(px(0.)).overflow_hidden().child(body))
            .into_any_element()
    }

    fn panel_shell_with_header(
        header: impl IntoElement,
        body: impl IntoElement,
        bg: gpui::Hsla,
        border: gpui::Hsla,
        title_bg: gpui::Hsla,
    ) -> AnyElement {
        v_flex()
            .size_full()
            .bg(bg)
            .border_1()
            .border_color(border)
            .child(
                h_flex()
                    .h(px(34.))
                    .items_center()
                    .px_2()
                    .bg(title_bg)
                    .border_b_1()
                    .border_color(border)
                    .child(header),
            )
            .child(div().flex_1().min_h(px(0.)).overflow_hidden().child(body))
            .into_any_element()
    }
}

impl Render for FiveChLayout {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx);
        // 現在のテーマ名を追跡する
        let current_theme_name = if theme.mode.is_dark() {
            theme.dark_theme.name.to_string()
        } else {
            theme.light_theme.name.to_string()
        };
        self.current_theme_name = Some(current_theme_name);

        // セッション復元: テーマの適用
        if let Some(name) = self.pending_restore_theme.take() {
            let config = ThemeRegistry::global(cx)
                .sorted_themes()
                .into_iter()
                .find(|t| t.name.as_ref() == name.as_str())
                .cloned();
            if let Some(config) = config {
                let theme_mode = config.mode;
                {
                    let theme = Theme::global_mut(cx);
                    if theme_mode.is_dark() {
                        theme.dark_theme = config;
                    } else {
                        theme.light_theme = config;
                    }
                }
                Theme::change(theme_mode, Some(window), cx);
                Theme::global_mut(cx).scrollbar_show = ScrollbarShow::Always;
            }
        }

        let theme = Theme::global(cx);
        let border = theme.border;
        let app_bg = theme.background;
        let panel_title_bg = theme.tab_bar;
        let panel_title_fg = theme.foreground;
        let board_bg = theme.sidebar;
        let board_fg = theme.sidebar_foreground;
        let list_bg_odd = theme.list;
        // let list_bg_even = theme.list_even;
        let res_name_fg = theme.primary;
        let res_time_fg = theme.muted_foreground;
        let res_id_fg = theme.primary;
        let body_plain_fg = theme.foreground;
        let body_url_fg = theme.link;
        let body_anchor_fg = theme.primary;
        let input_bg = theme.background;
        let input_fg = theme.foreground;

        // 空の本文確認後の投稿処理を実行する
        if self.post_pending_confirm_empty_body {
            self.post_pending_confirm_empty_body = false;
            self.execute_post_submission(window, cx);
        }

        if self.response_composer_open {
            self.ensure_response_composer_inputs(window, cx);
        }

        if let Some(y) = self.pending_restore_thread_scroll_y {
            if let Some(table_state) = &self.thread_table_state {
                table_state.update(cx, |state, _| {
                    state
                        .vertical_scroll_handle
                        .set_offset(point(px(0.), px(y.max(0.0))));
                });
                self.pending_restore_thread_scroll_y = None;
            }
        }
        if let Some(table_state) = &self.thread_table_state {
            if let Some(saved_columns) = self.pending_restore_thread_table_columns.take() {
                table_state.update(cx, |state, cx| {
                    // デフォルト列定義をキーで引けるマップを作る
                    let default_delegate = ThreadTableDelegate::new();
                    let default_map: std::collections::HashMap<&str, &Column> = default_delegate
                        .columns
                        .iter()
                        .map(|c| (c.key.as_ref(), c))
                        .collect();
                    // 保存順で並び替えた列リストを構築（デフォルトにない列はスキップ）
                    let mut restored: Vec<Column> = saved_columns
                        .iter()
                        .filter_map(|sc| {
                            default_map.get(sc.key.as_str()).map(|base| {
                                let mut col = (*base).clone();
                                col.width = px(sc.width_px);
                                col
                            })
                        })
                        .collect();
                    // 保存されていなかったデフォルト列を末尾に補完
                    let saved_keys: std::collections::HashSet<&str> =
                        saved_columns.iter().map(|sc| sc.key.as_str()).collect();
                    for base in &default_delegate.columns {
                        if !saved_keys.contains(base.key.as_ref()) {
                            restored.push(base.clone());
                        }
                    }
                    state.delegate_mut().columns = restored;
                    state.refresh(cx);
                    cx.notify();
                });
            }
        }
        if !self.responses.is_empty() && !self.has_placeholder_response_row() {
            if self.pending_restore_response_to_bottom {
                self.response_scroll_handle.scroll_to_item(
                    self.responses.len().saturating_sub(1),
                    ScrollStrategy::Bottom,
                );
                self.pending_restore_response_to_bottom = false;
                self.pending_restore_response_numbers = None;
                self.pending_restore_response_scroll_offset_y = None;
            } else if let Some(offset_y) = self.pending_restore_response_scroll_offset_y {
                self.response_scroll_handle
                    .set_offset(point(px(0.), px(offset_y.min(0.0))));
                self.pending_restore_response_scroll_offset_y = None;
                self.pending_restore_response_numbers = None;
            } else if let Some((top_number, bottom_number)) = self.pending_restore_response_numbers
            {
                if top_number.is_none() && bottom_number.is_none() {
                    self.pending_restore_response_numbers = None;
                } else {
                    let restore_ix = top_number
                        .and_then(|no| self.responses.iter().position(|res| res.number == no))
                        .or_else(|| {
                            bottom_number.and_then(|no| {
                                self.responses.iter().position(|res| res.number == no)
                            })
                        })
                        .or_else(|| {
                            top_number.and_then(|no| {
                                self.responses.iter().position(|res| res.number >= no)
                            })
                        })
                        .or_else(|| {
                            bottom_number.and_then(|no| {
                                self.responses.iter().rposition(|res| res.number <= no)
                            })
                        });

                    if let Some(ix) = restore_ix {
                        self.response_scroll_handle
                            .scroll_to_item(ix, ScrollStrategy::Top);
                    }
                    self.pending_restore_response_numbers = None;
                }
            }
        }

        let thread_scroll_y = self.current_thread_scroll_y(cx);
        if (self.last_thread_scroll_y - thread_scroll_y).abs() >= 1.0 {
            self.last_thread_scroll_y = thread_scroll_y;
            self.save_session_to_disk_debounced(cx);
        }

        let view = cx.entity();
        let board_tree = tree(&self.tree_state, move |ix, entry, selected, _window, cx| {
            let depth = entry.depth();
            let is_folder = entry.is_folder();
            let is_expanded = entry.is_expanded();
            let item_id = entry.item().id.clone();
            let item_label = entry.item().label.clone();
            view.update(cx, |_this, cx| {
                if is_folder {
                    let icon = if is_expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    };
                    ListItem::new(ix)
                        .w_full()
                        .pl(px(4.))
                        .selected(selected)
                        .child(
                            h_flex()
                                .gap_1()
                                .text_sm()
                                .text_color(board_fg)
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(icon)
                                .child(item_label),
                        )
                } else {
                    let item_url = item_id.clone();
                    let item_name = item_label.clone();
                    ListItem::new(ix)
                        .w_full()
                        .pl(px(16. * depth as f32 + 4.))
                        .selected(selected)
                        .child(div().text_sm().text_color(board_fg).child(item_label))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_board_tab(item_url.clone(), item_name.clone(), cx);
                        }))
                }
            })
        })
        .size_full()
        .text_color(board_fg);
        let sidebar_panel = Self::panel_shell(
            "板一覧",
            board_tree,
            board_bg,
            border,
            panel_title_bg,
            panel_title_fg,
        );

        let thread_table_state = self.ensure_thread_table_state(window, cx);
        let source_signature = self.current_thread_source_signature();
        if self.thread_table_source_signature != source_signature {
            self.thread_table_source_signature = source_signature;
            let (thread_rows, empty_message) = self.current_thread_rows();
            thread_table_state.update(cx, |state, cx| {
                state.delegate_mut().set_rows(thread_rows, empty_message);
                cx.notify();
            });
        }

        self.response_content_height_px = self
            .responses
            .iter()
            .map(|res| self.estimate_response_row_height_px(res))
            .sum::<f32>();

        let render_thread_panel = || {
            let selected_tab_ix = if self.thread_tabs.is_empty() {
                0
            } else {
                self.active_thread_tab
                    .min(self.thread_tabs.len().saturating_sub(1))
            };
            let thread_header_title = self.current_board_title();
            let tabs = self.thread_tabs.iter().enumerate().fold(
                TabBar::new("thread-tabs")
                    .small()
                    .selected_index(selected_tab_ix)
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        this.set_focused_pane(GesturePane::ThreadList);
                        this.active_thread_tab = *ix;
                        this.save_session_to_disk_debounced(cx);
                        cx.notify();
                    })),
                |bar, (tab_ix, tab)| {
                    bar.child(
                        Tab::new()
                            .label(tab.board_name.clone())
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    if event.click_count == 2 {
                                        this.reload_thread_tab(tab_ix, cx);
                                    }
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Middle,
                                cx.listener(move |this, _, _, cx| {
                                    this.close_thread_tab(tab_ix, cx);
                                }),
                            ),
                    )
                },
            );
            let thread_table = Table::new(&thread_table_state)
                .stripe(true)
                .bordered(false)
                .scrollbar_visible(true, false);

            Self::panel_shell_with_header(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .text_sm()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(panel_title_fg)
                            .child(thread_header_title),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new("thread-panel-refresh")
                                    .small()
                                    .ghost()
                                    .label("更新")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if !this.thread_tabs.is_empty() {
                                            let ix = this.active_thread_tab;
                                            this.reload_thread_tab(ix, cx);
                                        }
                                    })),
                            )
                            .child(
                                Button::new("thread-panel-close")
                                    .small()
                                    .ghost()
                                    .label("閉じる")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if !this.thread_tabs.is_empty() {
                                            let ix = this.active_thread_tab;
                                            this.close_thread_tab(ix, cx);
                                        }
                                    })),
                            ),
                    ),
                v_flex()
                    .size_full()
                    .child(div().border_b_1().border_color(border).child(tabs))
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_hidden()
                            .relative()
                            .child(thread_table)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event: &MouseDownEvent, _, _cx| {
                                    this.set_focused_pane(GesturePane::ThreadList);
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    this.set_focused_pane(GesturePane::ThreadList);
                                    this.begin_mouse_gesture(GesturePane::ThreadList, event, cx);
                                }),
                            )
                            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                                this.track_mouse_gesture_move(GesturePane::ThreadList, event, cx);
                            }))
                            .on_mouse_up(
                                MouseButton::Right,
                                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                                    let gesture_active =
                                        this.end_mouse_gesture(GesturePane::ThreadList, event, cx);
                                    if !gesture_active {
                                        this.open_thread_pane_context_menu(
                                            event.position,
                                            window,
                                            cx,
                                        );
                                    }
                                }),
                            )
                            .when(self.thread_pane_context_menu.open, |this| {
                                let menu_view = self.thread_pane_context_menu.menu_view.clone();
                                let position = self.thread_pane_context_menu.position;
                                this.child(
                                    deferred(
                                        anchored().child(
                                            anchored()
                                                .position(position)
                                                .snap_to_window_with_margin(px(8.))
                                                .anchor(Corner::TopLeft)
                                                .when_some(menu_view, |this, menu| {
                                                    this.child(menu.clone())
                                                }),
                                        ),
                                    )
                                    .with_priority(1),
                                )
                            }),
                    ),
                list_bg_odd,
                border,
                panel_title_bg,
            )
        };

        let render_response_panel = || {
            let response_header_title = self.current_response_title();
            let response_composer_open = self.response_composer_open;
            let response_form_name_state = self.response_form_name_state.clone();
            let response_form_mail_state = self.response_form_mail_state.clone();
            let response_form_body_state = self.response_form_body_state.clone();
            let response_form_sage = self.response_form_sage;
            let response_submit_hint = self.response_submit_hint.clone();
            let tabs = self.response_tabs.iter().enumerate().fold(
                TabBar::new("response-tabs")
                    .small()
                    .selected_index(self.active_response_tab)
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        this.set_focused_pane(GesturePane::ResponseList);
                        if let Some(tab) = this.response_tabs.get(*ix).cloned() {
                            this.open_thread(tab.board_url, tab.dat_file, tab.title, cx);
                        }
                    })),
                |bar, (tab_ix, tab)| {
                    bar.child(
                        Tab::new()
                            .label(Self::response_tab_label(tab.title.as_ref()))
                            .on_mouse_down(
                                MouseButton::Middle,
                                cx.listener(move |this, _, _, cx| {
                                    this.close_response_tab(tab_ix, cx);
                                }),
                            ),
                    )
                },
            );

            // 本文行数から各アイテムの高さを事前計算
            // py_1(8) + header(22) + gap_1(4) + body_lines(n×22) + border(1) = 35 + n*22
            let item_sizes: Rc<Vec<Size<Pixels>>> = Rc::new(
                self.responses
                    .iter()
                    .map(|res: &ResponseItem| {
                        size(px(1.), px(self.estimate_response_row_height_px(res)))
                    })
                    .collect(),
            );

            let response_rows = v_virtual_list(
                cx.entity(),
                "response-virtual-list",
                item_sizes,
                move |this, visible_range, _, cx| {
                    let visible_rows: Vec<(usize, ResponseItem)> = visible_range
                        .filter_map(|ix| this.responses.get(ix).cloned().map(|res| (ix, res)))
                        .collect();

                    let visible_top = visible_rows.first().map(|(_, res)| res.number);
                    let visible_bottom = visible_rows.last().map(|(_, res)| res.number);
                    let last_response_number = this.responses.last().map(|res| res.number);
                    let is_at_bottom = match (visible_bottom, last_response_number) {
                        (Some(visible), Some(last)) => visible >= last,
                        _ => false,
                    };
                    this.current_response_was_at_bottom = is_at_bottom;
                    if let Some(tab) = this.response_tabs.get_mut(this.active_response_tab) {
                        tab.was_at_bottom = is_at_bottom;
                    }
                    let should_track_visible_numbers =
                        !this.has_placeholder_response_row()
                            && this.pending_restore_response_numbers.is_none()
                            && this.pending_restore_response_scroll_offset_y.is_none()
                            && !this.pending_restore_response_to_bottom;
                    if should_track_visible_numbers {
                        if this.current_response_top_number != visible_top
                            || this.current_response_bottom_number != visible_bottom
                        {
                            this.current_response_top_number = visible_top;
                            this.current_response_bottom_number = visible_bottom;
                            if let Some(tab) = this.response_tabs.get_mut(this.active_response_tab) {
                                tab.top_response_number = visible_top;
                                tab.bottom_response_number = visible_bottom;
                            }
                            this.save_session_to_disk_debounced(cx);
                        }
                    }

                    visible_rows
                        .into_iter()
                        .map(|(_ix, res)| {
                            let is_newly_added = this
                                .response_new_marker_from
                                .is_some_and(|from_no| res.number >= from_no);
                            // コンテキストメニュー用にテキストを事前抽出
                            let copy_body = res.body.to_string();
                            let copy_name = response_name_plain_text(res.name.as_ref());
                            let copy_id = res.id.to_string();
                            let copy_timestamp = res.timestamp.to_string();
                            let copy_number = res.number;
                            let displayed_urls = Self::extract_displayed_urls(res.body.as_ref());
                            let copy_full = format!(
                                "{} {} {}
{}
{}",
                                copy_number, copy_name, copy_timestamp, copy_id, copy_body
                            );

                            let body_lines = res.body.lines().enumerate().fold(
                                v_flex().w_full().gap_0p5(),
                                |body, (line_ix, line)| {
                                    let line_row = Self::split_body_tokens(line)
                                        .into_iter()
                                        .enumerate()
                                        .fold(
                                        h_flex().w_full().min_w_0().flex_wrap().gap_0(),
                                        |row, (token_ix, (kind, token))| {
                                            let color = match kind {
                                                BodyTokenKind::Plain => body_plain_fg,
                                                BodyTokenKind::Url => body_url_fg,
                                                BodyTokenKind::Anchor => body_anchor_fg,
                                            };
                                            match kind {
                                                BodyTokenKind::Url => {
                                                    let url = token.trim().to_string();
                                                    if Self::is_image_preview_url(&url) {
                                                        let image_link_id: SharedString = format!(
                                                            "response-image-url-{}-{}-{}",
                                                            res.number, line_ix, token_ix
                                                        )
                                                        .into();
                                                        let url_for_open = url.clone();
                                                        let url_for_hover = url.clone();
                                                        let url_for_move = url.clone();
                                                        row.child(
                                                            div()
                                                                .id(image_link_id)
                                                                .min_w_0()
                                                                .text_sm()
                                                                .text_color(color)
                                                                .underline()
                                                                .cursor_pointer()
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    cx.listener(move |this, _, _, cx| {
                                                                        let url_clone = url_for_open.clone();
                                                                        if !this.open_5ch_io_url_in_response_tab(&url_clone, cx) {
                                                                            Self::open_url_in_external_browser(&url_clone);
                                                                        }
                                                                    }),
                                                                )
                                                                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                                                    if *hovered {
                                                                        this.begin_image_preview_for_url(
                                                                            url_for_hover.clone(),
                                                                            point(px(0.), px(0.)),
                                                                            cx,
                                                                        );
                                                                    } else {
                                                                        this.clear_image_preview_for_url(&url_for_hover, cx);
                                                                    }
                                                                }))
                                                                .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                                                                    this.begin_image_preview_for_url(
                                                                        url_for_move.clone(),
                                                                        event.position,
                                                                        cx,
                                                                    );
                                                                    this.update_image_preview_position(
                                                                        &url_for_move,
                                                                        event.position,
                                                                        cx,
                                                                    );
                                                                }))
                                                                .child(token),
                                                        )
                                                    } else {
                                                        row.child(
                                                            div()
                                                                .min_w_0()
                                                                .text_sm()
                                                                .text_color(color)
                                                                .underline()
                                                                .cursor_pointer()
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    cx.listener(move |this, _, _, cx| {
                                                                        let url_clone = url.clone();
                                                                        if !this.open_5ch_io_url_in_response_tab(&url_clone, cx) {
                                                                            Self::open_url_in_external_browser(&url_clone);
                                                                        }
                                                                    }),
                                                                )
                                                                .child(token),
                                                        )
                                                    }
                                                }
                                                _ => row.child(
                                                    div()
                                                        .min_w_0()
                                                        .text_sm()
                                                        .text_color(color)
                                                        .child(token),
                                                ),
                                            }
                                        },
                                    );
                                    body.child(line_row)
                                },
                            );
                            v_flex()
                                .w_full()
                                .px_3()
                                .py_1()
                                .gap_1()
                                // .bg(if ix % 2 == 0 {
                                //     list_bg_even
                                // } else {
                                //     list_bg_odd
                                // })
                                .border_t_1()
                                .border_color(border)
                                .child(
                                    h_flex()
                                        .w_full()
                                        .min_w_0()
                                        .flex_wrap()
                                        .gap_2()
                                        .child(
                                            h_flex()
                                                .min_w_0()
                                                .flex_wrap()
                                                .gap_0()
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(body_plain_fg)
                                                        .text_xs()
                                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                                        .child(if is_newly_added { "新着 " } else { "" }),
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(res_name_fg)
                                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                                        .child(format!("{:>4} ", res.number)),
                                                )
                                                .children(
                                                    split_response_name_tokens(res.name.as_ref())
                                                        .into_iter()
                                                        .map(|(is_bold, token)| {
                                                            div()
                                                                .min_w_0()
                                                                .text_sm()
                                                                .text_color(res_name_fg)
                                                                .font_weight(if is_bold {
                                                                    gpui::FontWeight::SEMIBOLD
                                                                } else {
                                                                    gpui::FontWeight::NORMAL
                                                                })
                                                                .child(token)
                                                        })
                                                        .collect::<Vec<_>>(),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .min_w_0()
                                                .text_sm()
                                                .text_color(res_time_fg)
                                                .child(res.timestamp.clone()),
                                        )
                                        .child(
                                            div()
                                                .min_w_0()
                                                .text_sm()
                                                .text_color(res_id_fg)
                                                .child(res.id.clone()),
                                        ),
                                )
                                .child(body_lines)
                                .on_mouse_up(
                                    MouseButton::Right,
                                    cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                                        let gesture_active = this
                                            .end_mouse_gesture(GesturePane::ResponseList, event, cx);
                                        if !gesture_active {
                                            this.open_response_row_context_menu(
                                                event.position,
                                                copy_body.clone(),
                                                copy_name.clone(),
                                                copy_id.clone(),
                                                copy_full.clone(),
                                                displayed_urls.clone(),
                                                window,
                                                cx,
                                            );
                                        }
                                    }),
                                )
                        })
                        .collect::<Vec<_>>()
                },
            )
            .track_scroll(&self.response_scroll_handle)
            .size_full();

            Self::panel_shell_with_header(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .text_sm()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(panel_title_fg)
                            .child(response_header_title),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new("response-panel-compose-toggle")
                                    .small()
                                    .ghost()
                                    .label(if response_composer_open {
                                        "書き込みを閉じる"
                                    } else {
                                        "書き込み"
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_response_composer(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("response-panel-refresh")
                                    .small()
                                    .ghost()
                                    .label("更新")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.reload_current_thread(cx);
                                    })),
                            )
                            .child(
                                Button::new("response-panel-close")
                                    .small()
                                    .ghost()
                                    .label("閉じる")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close_current_thread(cx);
                                    })),
                            ),
                    ),
                v_flex()
                    .size_full()
                    .child(div().border_b_1().border_color(border).child(tabs))
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_hidden()
                            .relative()
                            .child(response_rows)
                            .child(
                                deferred(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .right_0()
                                        .bottom_0()
                                        .child(
                                            Scrollbar::vertical(&self.response_scroll_handle)
                                                .scrollbar_show(ScrollbarShow::Always),
                                        ),
                                )
                                .with_priority(5),
                            )
                            .when(self.response_row_context_menu.open, |this| {
                                let menu_view = self.response_row_context_menu.menu_view.clone();
                                let position = self.response_row_context_menu.position;
                                this.child(
                                    deferred(
                                        anchored().child(
                                            anchored()
                                                .position(position)
                                                .snap_to_window_with_margin(px(8.))
                                                .anchor(Corner::TopLeft)
                                                .when_some(menu_view, |this, menu| {
                                                    this.child(menu.clone())
                                                }),
                                        ),
                                    )
                                    .with_priority(1),
                                )
                            })
                            .when_some(self.image_preview_state.clone(), |this, preview| {
                                let (panel_width, panel_height, panel) = match preview.status {
                                    ImagePreviewStatus::Loading => (
                                        px(320.),
                                        px(220.),
                                        div()
                                            .w(px(320.))
                                            .h(px(220.))
                                            // .p_3()
                                            .items_center()
                                            .justify_center()
                                            .text_sm()
                                            .text_color(panel_title_fg)
                                            .child("画像を読み込み中..."),
                                    ),
                                    ImagePreviewStatus::Ready { local_path } => (
                                        px(480.),
                                        px(320.),
                                        div()
                                            .w(px(480.))
                                            .h(px(320.))
                                            // .p_2()
                                            .child(
                                                img(local_path)
                                                    .size_full()
                                                    .object_fit(ObjectFit::Contain)
                                                    .with_loading(|| {
                                                        div()
                                                            .size_full()
                                                            .items_center()
                                                            .justify_center()
                                                            .text_sm()
                                                            .child("画像を読み込み中...")
                                                            .into_any_element()
                                                    })
                                                    .with_fallback(|| {
                                                        div()
                                                            .size_full()
                                                            .items_center()
                                                            .justify_center()
                                                            .text_sm()
                                                            .child("画像を表示できません")
                                                            .into_any_element()
                                                    }),
                                            ),
                                    ),
                                    ImagePreviewStatus::Error { message } => (
                                        px(360.),
                                        px(120.),
                                        div()
                                            .w(px(360.))
                                            .h(px(120.))
                                            // .p_3()
                                            .items_center()
                                            .justify_center()
                                            .text_sm()
                                            .text_color(panel_title_fg)
                                            .child(message),
                                    ),
                                };

                                let preview_offset = px(16.);
                                let preview_margin = px(12.);
                                let place_above = preview.position.y
                                    > panel_height + preview_offset + preview_margin;
                                let place_left = preview.position.x
                                    > panel_width + preview_offset + preview_margin;

                                let popup_anchor = match (place_above, place_left) {
                                    (true, true) => Corner::BottomRight,
                                    (true, false) => Corner::BottomLeft,
                                    (false, true) => Corner::TopRight,
                                    (false, false) => Corner::TopLeft,
                                };

                                let popup_position = point(
                                    if place_left {
                                        preview.position.x - preview_offset
                                    } else {
                                        preview.position.x + preview_offset
                                    },
                                    if place_above {
                                        preview.position.y - preview_offset
                                    } else {
                                        preview.position.y + preview_offset
                                    },
                                );

                                this.child(
                                    deferred(
                                        anchored().child(
                                            anchored()
                                                .position(popup_position)
                                                .snap_to_window_with_margin(px(12.))
                                                .anchor(popup_anchor)
                                                .child(
                                                    div()
                                                        .rounded_lg()
                                                        .border_1()
                                                        .border_color(border)
                                                        .bg(panel_title_bg)
                                                        .child(panel),
                                                ),
                                        ),
                                    )
                                    .with_priority(2),
                                )
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event: &MouseDownEvent, _, _cx| {
                                    this.set_focused_pane(GesturePane::ResponseList);
                                }),
                            )
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    this.set_focused_pane(GesturePane::ResponseList);
                                    this.begin_mouse_gesture(GesturePane::ResponseList, event, cx);
                                }),
                            )
                            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                                this.track_mouse_gesture_move(GesturePane::ResponseList, event, cx);
                            }))
                            .on_mouse_up(
                                MouseButton::Right,
                                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                                    _ = this.end_mouse_gesture(
                                        GesturePane::ResponseList,
                                        event,
                                        cx,
                                    );
                                }),
                            )
                            .on_children_prepainted({
                                let view = cx.entity();
                                move |bounds, _window, cx| {
                                    if let Some(first) = bounds.first() {
                                        let measured_w = f32::from(first.size.width);
                                        let measured_h = f32::from(first.size.height);
                                        _ = view.update(cx, |this, cx| {
                                            let width_changed =
                                                (this.response_virtual_body_width_px - measured_w)
                                                    .abs()
                                                    >= 1.0;
                                            let height_changed = (this.response_viewport_height_px
                                                - measured_h)
                                                .abs()
                                                >= 1.0;
                                            if width_changed || height_changed {
                                                this.response_virtual_body_width_px = measured_w;
                                                this.response_viewport_height_px = measured_h;
                                                cx.notify();
                                            }
                                        });
                                    }
                                }
                            }),
                    )
                    .child(if response_composer_open {
                        v_flex()
                            .w_full()
                            .h(px(250.))
                            .border_t_1()
                            .border_color(border)
                            .bg(list_bg_odd)
                            .child(
                                h_flex()
                                    .h(px(40.))
                                    .items_center()
                                    .gap_2()
                                    .px_3()
                                    .border_b_1()
                                    .border_color(border)
                                    .child(
                                        div().h(px(30.)).w(px(220.)).child(
                                            response_form_name_state
                                                .clone()
                                                .map(|state| {
                                                    Input::new(&state)
                                                        .small()
                                                        .w_full()
                                                        .h_full()
                                                        .bg(input_bg)
                                                        .into_any_element()
                                                })
                                                .unwrap_or_else(|| div().into_any_element()),
                                        ),
                                    )
                                    .child(
                                        div().h(px(30.)).w(px(220.)).child(
                                            response_form_mail_state
                                                .clone()
                                                .map(|state| {
                                                    Input::new(&state)
                                                        .small()
                                                        .w_full()
                                                        .h_full()
                                                        .bg(input_bg)
                                                        .into_any_element()
                                                })
                                                .unwrap_or_else(|| div().into_any_element()),
                                        ),
                                    )
                                    .child(
                                        Checkbox::new("response-panel-sage-toggle")
                                            .small()
                                            .label("sage")
                                            .checked(response_form_sage)
                                            .on_click(cx.listener(
                                                |this, checked: &bool, _, cx| {
                                                    this.response_form_sage = *checked;
                                                    cx.notify();
                                                },
                                            )),
                                    ),
                            )
                            .child(
                                div().flex_1().min_h(px(0.)).p_3().child(
                                    response_form_body_state
                                        .clone()
                                        .map(|state| {
                                            Input::new(&state)
                                                .small()
                                                .w_full()
                                                .h_full()
                                                .bg(input_bg)
                                                .into_any_element()
                                        })
                                        .unwrap_or_else(|| div().into_any_element()),
                                ),
                            )
                            .child(
                                h_flex()
                                    .h(px(40.))
                                    .items_center()
                                    .justify_between()
                                    .px_3()
                                    .border_t_1()
                                    .border_color(border)
                                    .child(
                                        Button::new("response-panel-submit-dummy")
                                            .small()
                                            .ghost()
                                            .label("書き込む")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.submit_response_draft(window, cx);
                                            })),
                                    )
                                    .child(div().text_sm().text_color(input_fg).child(
                                        response_submit_hint.clone().unwrap_or_else(|| " ".into()),
                                    )),
                            )
                    } else {
                        div().h(px(0.))
                    }),
                list_bg_odd,
                border,
                panel_title_bg,
            )
        };

        let view_for_center = cx.entity().downgrade();
        let center_split_size = self.center_split_size;
        let center_panes = match self.layout_mode {
            LayoutMode::Horizontal => {
                let view_c = view_for_center.clone();
                h_resizable("center-horizontal")
                    .child(
                        resizable_panel()
                            .size(px(center_split_size))
                            .size_range(px(240.)..px(820.))
                            .child(render_thread_panel()),
                    )
                    .child(resizable_panel().child(render_response_panel()))
                    .on_resize(move |state, _window, cx| {
                        let sizes = state.read(cx).sizes().clone();
                        if let Some(this) = view_c.upgrade() {
                            _ = this.update(cx, |layout, _cx| {
                                if let Some(&s) = sizes.first() {
                                    layout.center_split_size = f32::from(s);
                                }
                                layout.save_session_to_disk_debounced(_cx);
                            });
                        }
                    })
                    .into_any_element()
            }
            LayoutMode::Vertical => {
                let view_c = view_for_center.clone();
                v_resizable("center-vertical")
                    .child(
                        resizable_panel()
                            .size(px(center_split_size))
                            .size_range(px(160.)..px(600.))
                            .child(render_thread_panel()),
                    )
                    .child(resizable_panel().child(render_response_panel()))
                    .on_resize(move |state, _window, cx| {
                        let sizes = state.read(cx).sizes().clone();
                        if let Some(this) = view_c.upgrade() {
                            _ = this.update(cx, |layout, _cx| {
                                if let Some(&s) = sizes.first() {
                                    layout.center_split_size = f32::from(s);
                                }
                                layout.save_session_to_disk_debounced(_cx);
                            });
                        }
                    })
                    .into_any_element()
            }
            LayoutMode::Single => {
                // スレッドが開いていればレス一覧、そうでなければスレッド一覧を表示
                if self.current_thread.is_some() {
                    render_response_panel().into_any_element()
                } else {
                    render_thread_panel().into_any_element()
                }
            }
        };

        let header = TitleBar::new()
            .bg(panel_title_bg)
            .child(
                Button::new("app-menu")
                    .icon(IconName::Menu)
                    .ghost()
                    .small()
                    .dropdown_menu({
                        let view_theme = cx.entity().downgrade();
                        move |menu, _window, cx| {
                            // テーマ一覧を収集
                            let themes: Vec<SharedString> = ThemeRegistry::global(cx)
                                .sorted_themes()
                                .iter()
                                .filter(|t| t.name != "Default Dark" && t.name != "Default Light")
                                .map(|t| t.name.clone())
                                .collect();

                            let menu = menu.item(PopupMenuItem::label("テーマ"));
                            let menu = themes.into_iter().fold(menu, |m, name| {
                                let name_clone = name.clone();
                                let view_t = view_theme.clone();
                                m.item(PopupMenuItem::new(name).on_click(move |_, window, cx| {
                                    let config = ThemeRegistry::global(cx)
                                        .sorted_themes()
                                        .into_iter()
                                        .find(|t| t.name == name_clone)
                                        .cloned();
                                    if let Some(config) = config {
                                        let theme_mode = config.mode;
                                        let applied_name = config.name.to_string();
                                        {
                                            let theme = Theme::global_mut(cx);
                                            if theme_mode.is_dark() {
                                                theme.dark_theme = config;
                                            } else {
                                                theme.light_theme = config;
                                            }
                                        }
                                        Theme::change(theme_mode, Some(window), cx);
                                        if let Some(this) = view_t.upgrade() {
                                            _ = this.update(cx, |layout, _cx| {
                                                layout.current_theme_name = Some(applied_name);
                                                layout.save_session_to_disk_debounced(_cx);
                                            });
                                        }
                                    }
                                }))
                            });
                            menu.separator()
                                .item(PopupMenuItem::new("終了").on_click(|_, _, cx| cx.quit()))
                        }
                    }),
            )
            .child(
                div()
                    .text_color(panel_title_fg)
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_sm()
                    .child("Gooey"),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(if self.layout_mode == LayoutMode::Horizontal {
                        Button::new("mode-h")
                            .small()
                            .primary()
                            .label("横並び")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_layout_mode(LayoutMode::Horizontal, cx);
                            }))
                    } else {
                        Button::new("mode-h")
                            .small()
                            .ghost()
                            .label("横並び")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_layout_mode(LayoutMode::Horizontal, cx);
                            }))
                    })
                    .child(if self.layout_mode == LayoutMode::Vertical {
                        Button::new("mode-v")
                            .small()
                            .primary()
                            .label("縦並び")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_layout_mode(LayoutMode::Vertical, cx);
                            }))
                    } else {
                        Button::new("mode-v")
                            .small()
                            .ghost()
                            .label("縦並び")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_layout_mode(LayoutMode::Vertical, cx);
                            }))
                    })
                    .child(if self.layout_mode == LayoutMode::Single {
                        Button::new("mode-s")
                            .small()
                            .primary()
                            .label("単独")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_layout_mode(LayoutMode::Single, cx);
                            }))
                    } else {
                        Button::new("mode-s")
                            .small()
                            .ghost()
                            .label("単独")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_layout_mode(LayoutMode::Single, cx);
                            }))
                    }),
            );

        let footer = h_flex()
            .h(px(28.))
            .items_center()
            .justify_between()
            .px_3()
            .bg(panel_title_bg)
            .border_t_1()
            .border_color(border)
            .text_sm()
            .text_color(panel_title_fg)
            .child({
                let mut text: String = format!(
                    "Focused:{:?} {}",
                    self.focused_pane, self.gesture_trail_text
                );
                if let Some(hint) = &self.pull_refresh_hint {
                    text.push_str(" / ");
                    text.push_str(hint.as_ref());
                }
                if let Some(hint) = &self.footer_status_hint {
                    text.push_str(" / ");
                    text.push_str(hint.as_ref());
                }
                text
            })
            .child(format!(
                "ThreadTab: {}  ResponseTab: {}",
                if self.thread_tabs.is_empty() {
                    0
                } else {
                    self.active_thread_tab + 1
                },
                if self.response_tabs.is_empty() {
                    0
                } else {
                    self.active_response_tab + 1
                }
            ));

        let view_for_keys = cx.entity();
        let view_for_wheel = cx.entity();

        v_flex()
            .size_full()
            .bg(app_bg)
            .text_color(body_plain_fg)
            .on_key_down(move |event: &KeyDownEvent, _window, cx| {
                let key = event.keystroke.key.clone();
                let control = event.keystroke.modifiers.control;
                _ = view_for_keys.update(cx, |this, cx| {
                    this.handle_key_binding_key(&key, control, cx);
                });
            })
            .on_scroll_wheel(move |event: &ScrollWheelEvent, _window, cx| {
                _ = view_for_wheel.update(cx, |this, cx| {
                    this.handle_pull_refresh_wheel(event, cx);
                });
            })
            .child(header)
            .child(
                div().flex_1().min_h(px(0.)).child(
                    h_resizable("workspace")
                        .child(
                            resizable_panel()
                                .size(px(self.workspace_sidebar_width))
                                .size_range(px(120.)..px(450.))
                                .child(sidebar_panel),
                        )
                        .child(resizable_panel().child(center_panes))
                        .on_resize({
                            let view_w = cx.entity().downgrade();
                            move |state, _window, cx| {
                                let sizes = state.read(cx).sizes().clone();
                                if let Some(this) = view_w.upgrade() {
                                    _ = this.update(cx, |layout, _cx| {
                                        if let Some(&s) = sizes.first() {
                                            layout.workspace_sidebar_width = f32::from(s);
                                        }
                                        layout.save_session_to_disk_debounced(_cx);
                                    });
                                }
                            }
                        }),
                ),
            )
            .child(footer)
            .child({
                // モーダルオーバーレイレイヤー
                if let Some(modal) = &self.modal_state {
                    let overlay_bg = gpui::Hsla {
                        h: 0.0,
                        s: 0.0,
                        l: 0.0,
                        a: 0.5,
                    };
                    let theme = Theme::global(cx);
                    let border = theme.border;
                    let panel_bg = theme.background;
                    let panel_fg = theme.foreground;

                    let content = match modal {
                        Modal::Confirmation {
                            title,
                            message,
                            ok_label,
                            cancel_label,
                        } => {
                            let title_clone = title.clone();
                            let message_clone = message.clone();
                            let ok_label_clone = ok_label.clone();
                            let cancel_label_clone = cancel_label.clone();

                            div()
                                .absolute()
                                .inset_0()
                                .bg(overlay_bg)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .w(px(400.))
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(border)
                                        .bg(panel_bg)
                                        .flex_col()
                                        .child(
                                            div()
                                                .px_4()
                                                .py_3()
                                                .border_b_1()
                                                .border_color(border)
                                                .text_base()
                                                .font_weight(gpui::FontWeight::BOLD)
                                                .text_color(panel_fg)
                                                .child(title_clone),
                                        )
                                        .child(
                                            div()
                                                .px_4()
                                                .py_4()
                                                .text_sm()
                                                .text_color(panel_fg)
                                                .child(message_clone),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .px_4()
                                                .py_3()
                                                .border_t_1()
                                                .border_color(border)
                                                .justify_end()
                                                .child(
                                                    Button::new("modal-cancel")
                                                        .small()
                                                        .ghost()
                                                        .label(cancel_label_clone)
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.close_modal(cx);
                                                        })),
                                                )
                                                .child(
                                                    Button::new("modal-ok")
                                                        .small()
                                                        .primary()
                                                        .label(ok_label_clone)
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            // 空の本文確認ダイアログの場合、投稿を続行
                                                            this.post_pending_confirm_empty_body =
                                                                true;
                                                            this.close_modal(cx);
                                                        })),
                                                ),
                                        ),
                                )
                                .into_any_element()
                        }
                        Modal::Alert { title, message } => {
                            let title_clone = title.clone();
                            let message_clone = message.clone();

                            div()
                                .absolute()
                                .inset_0()
                                .bg(overlay_bg)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .w(px(400.))
                                        .rounded_lg()
                                        .border_1()
                                        .border_color(border)
                                        .bg(panel_bg)
                                        .flex_col()
                                        .child(
                                            div()
                                                .px_4()
                                                .py_3()
                                                .border_b_1()
                                                .border_color(border)
                                                .text_base()
                                                .font_weight(gpui::FontWeight::BOLD)
                                                .text_color(panel_fg)
                                                .child(title_clone),
                                        )
                                        .child(
                                            div()
                                                .px_4()
                                                .py_4()
                                                .text_sm()
                                                .text_color(panel_fg)
                                                .child(message_clone),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .px_4()
                                                .py_3()
                                                .border_t_1()
                                                .border_color(border)
                                                .justify_end()
                                                .child(
                                                    Button::new("modal-ok-alert")
                                                        .small()
                                                        .primary()
                                                        .label("OK")
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.close_modal(cx);
                                                        })),
                                                ),
                                        ),
                                )
                                .into_any_element()
                        }
                    };

                    content
                } else {
                    div().into_any_element()
                }
            })
    }
}

fn main() {
    let app = Application::new().with_assets(AppAssets::new());
    env_logger::init();

    app.run(move |cx| {
        gpui_component::init(cx);
        let themes_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("themes");
        if let Err(err) = ThemeRegistry::watch_dir(themes_dir, cx, |_cx| {}) {
            warn!("カスタムテーマの監視開始に失敗しました: {err}");
        }

        cx.spawn(async move |cx| {
            let window_options = WindowOptions {
                titlebar: Some(TitleBar::title_bar_options()),
                ..Default::default()
            };

            cx.open_window(window_options, |window, cx| {
                window.set_window_title("Gooey - 5ch Browser Mock");
                let view = FiveChLayout::view(window, cx);
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("failed to open window");
        })
        .detach();
    });
}
