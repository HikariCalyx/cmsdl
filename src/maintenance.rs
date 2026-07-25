//! Maintenance notice fetcher for CMS and TMS regions.
//!
//! - `cmsdl cms --maintenance`: fetches from the CMS news API.
//! - `cmsdl tms --maintenance`: fetches from the TMS bulletin API (via CSRF).
//!
//! Supports three output modes:
//! - **Terminal** (default): ANSI-colored plain text.
//! - **JSON** (`--json`): structured JSON with a markdown-formatted body.
//! - **Discord** (`--json --discord`): JSON body includes ANSI color codes
//!   for use inside Discord ```ansi … ``` code blocks.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::cli::Region;

const VERSION: &str = env!("CARGO_PKG_VERSION");

// ── CMS API ─────────────────────────────────────────────────────────────────

/// URL of the CMS in-game news list API.
const CMS_NEWS_LIST_URL: &str =
    "https://news-my.web.sdo.com/UniInterface/data/mxd_news_list_ingame.ashx";

/// URL template for fetching a single CMS news article's content.
const CMS_NEWS_CONTENT_URL: &str = "https://mxd.web.sdo.com/web8/Handler/NewsContent.ashx?id=";

// ── TMS API ─────────────────────────────────────────────────────────────────

/// TMS main page (used to obtain the CSRF token/cookie pair).
const TMS_MAIN_URL: &str = "https://maplestory.beanfun.com/main";

/// TMS bulletin list endpoint.
const TMS_BULLETIN_LIST_URL: &str = "https://maplestory.beanfun.com/main?handler=BulletinProxy";

/// TMS bulletin detail endpoint.
const TMS_BULLETIN_DETAIL_URL: &str =
    "https://maplestory.beanfun.com/bulletin?handler=BulletinDetail";

/// Output mode for the rendered body.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RenderMode {
    /// Plain text with ANSI terminal escape codes.
    Terminal,
    /// Markdown-formatted plain text (no ANSI codes).
    Markdown,
    /// Markdown-formatted text with embedded ANSI color codes (for Discord).
    Discord,
    /// ANSI colour + bold codes, for GUI rendering (GDI parses ANSI codes).
    Gui,
}

/// An entry in the news list.
#[derive(Deserialize, Debug)]
struct NewsItem {
    id: u64,
    title: String,
    #[allow(dead_code)]
    pubdate: String,
    #[allow(dead_code)]
    url: String,
    #[allow(dead_code)]
    #[serde(default)]
    style: String,
}

/// A category group in the news list response.
#[derive(Deserialize, Debug)]
struct NewsCategory {
    #[allow(dead_code)]
    title: String,
    data: Vec<NewsItem>,
}

/// Response from the news content API.
#[derive(Deserialize, Debug)]
struct NewsContentResponse {
    #[allow(dead_code)]
    #[serde(rename = "Result")]
    result: i32,
    #[allow(dead_code)]
    #[serde(rename = "Message")]
    message: String,
    data: NewsContentData,
}

/// The `data` field within the news content response.
#[derive(Deserialize, Debug)]
struct NewsContentData {
    #[serde(rename = "ID")]
    #[allow(dead_code)]
    id: String,
    #[serde(rename = "Title")]
    title: String,
    #[serde(rename = "Content")]
    content: String,
    #[serde(rename = "PublishDate")]
    publish_date: String,
}

// ── TMS data structures ─────────────────────────────────────────────────────

/// Response envelope for TMS bulletin list & detail.
#[derive(Deserialize, Debug)]
struct TmsResponse {
    data: TmsData,
}

#[derive(Deserialize, Debug)]
struct TmsData {
    #[serde(rename = "myDataSet")]
    my_data_set: TmsDataSet,
}

#[derive(Deserialize, Debug)]
struct TmsDataSet {
    table: serde_json::Value,
}

/// An item in the TMS bulletin list.
#[derive(Deserialize, Debug, Clone)]
struct TmsBulletinItem {
    #[serde(rename = "bullentinId")]
    bulletin_id: String,
    title: String,
    #[serde(rename = "startDate")]
    start_date: String,
    #[serde(default)]
    #[serde(rename = "endDate")]
    #[allow(dead_code)]
    end_date: Option<String>,
}

/// TMS bulletin detail (single record returned as a dict).
#[derive(Deserialize, Debug)]
struct TmsBulletinDetail {
    #[serde(rename = "bullentinId")]
    #[allow(dead_code)]
    bulletin_id: String,
    title: String,
    content: String,
    #[serde(rename = "startDate")]
    start_date: String,
}

/// JSON output structure for `--json` mode.
#[derive(Serialize)]
struct MaintenanceOutput {
    id: u64,
    title: String,
    /// Unix timestamp (seconds since epoch) of the publish date (UTC+8 → UTC).
    publish_date: i64,
    body: String,
}

/// Fetch and display the most recent maintenance notice for the given region.
pub fn show_maintenance(
    agent: &ureq::Agent,
    region: Region,
    json: bool,
    discord: bool,
) -> Result<()> {
    match region {
        Region::Cms => show_cms_maintenance(agent, json, discord),
        Region::Tms => show_tms_maintenance(agent, json, discord),
        Region::Manual => bail!("--maintenance is not supported for region 'manual'"),
    }
}

/// Fetch the maintenance notice for GUI display, returning `(title, body, date)`
/// where `date` is a normalised `YYYY-MM-DD` string.
/// If `maint_id` is set, that specific bulletin is fetched instead of
/// searching for the latest maintenance notice.
/// Silently returns `None` on any error.
pub fn fetch_for_gui(agent: &ureq::Agent, region: Region, maint_id: Option<u64>) -> Option<(String, String, String)> {
    let result = match region {
        Region::Cms => fetch_cms_for_gui(agent, maint_id),
        Region::Tms => fetch_tms_for_gui(agent, maint_id),
        Region::Manual => return None,
    };
    result.ok()
}

fn fetch_cms_for_gui(agent: &ureq::Agent, maint_id: Option<u64>) -> Result<(String, String, String)> {
    let id = if let Some(mid) = maint_id {
        mid
    } else {
        let news_list = fetch_cms_news_list(agent)?;
        let item = find_cms_maintenance(&news_list)?;
        item.id
    };
    let content = fetch_cms_news_content(agent, id)?;
    let body = render_html(&content.content, RenderMode::Gui);
    let body = collapse_blank_lines(&body);
    let body = localize_stroke_out(&body);
    let date = content.publish_date.split_whitespace().next().unwrap_or(&content.publish_date).to_string();
    Ok((content.title, body, date))
}

fn fetch_tms_for_gui(agent: &ureq::Agent, maint_id: Option<u64>) -> Result<(String, String, String)> {
    let csrf_token = acquire_tms_csrf(agent)?;
    let (bid, items_opt) = if let Some(mid) = maint_id {
        (mid.to_string(), None)
    } else {
        let items = fetch_tms_bulletins(agent, &csrf_token, 5)?;
        let main_item = items
            .iter()
            .filter(|i| i.title.contains("維護公告"))
            .max_by_key(|i| &i.start_date)
            .ok_or_else(|| anyhow::anyhow!("no maintenance announcement"))?;
        (main_item.bulletin_id.clone(), Some((items.clone(), main_item.start_date.clone())))
    };
    let detail = fetch_tms_bulletin_detail(agent, &csrf_token, &bid)?;
    let mut body = render_html(&detail.content, RenderMode::Gui);
    // Check for delayed-opening notice (only when searching, not with --maintid).
    if let Some((items, maint_date)) = items_opt {
        if let Some(d) = items.iter().find(|i| {
            i.title.contains("延後開機公告") && i.start_date >= maint_date
        }) {
            if let Ok(dd) = fetch_tms_bulletin_detail(agent, &csrf_token, &d.bulletin_id) {
                body.push_str("\n\n---\n\n");
                body.push_str(&dd.title);
                body.push_str("\n\n");
                body.push_str(&render_html(&dd.content, RenderMode::Gui));
            }
        }
    }
    let body = collapse_blank_lines(&body);
    let body = localize_stroke_out(&body);
    // Normalise TMS date format: "2026/07/23" → "2026-07-23"
    let date = detail.start_date.replace('/', "-");
    Ok((detail.title, body, date))
}

/// Format a date string `YYYY-MM-DD` for GUI display using the current locale.
pub fn fmt_gui_date(date_str: &str) -> String {
    let d = match chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => return date_str.to_string(),
    };
    // Detect CJK locale by sampling a known translation.
    let sample = crate::locale::tr("gui-click-to-expand", &[]);
    let is_cjk = sample.contains(|c: char| c >= '\u{4E00}' && c <= '\u{9FFF}');
    if is_cjk {
        let y = d.format("%Y").to_string();
        let m = d.format("%m").to_string().trim_start_matches('0').to_string();
        let d2 = d.format("%d").to_string().trim_start_matches('0').to_string();
        format!("{y}年{m}月{d2}日")
    } else {
        d.format("%b %d, %Y").to_string()
    }
}

/// Replace hard-coded "(stroke out)" with the localised equivalent.
fn localize_stroke_out(body: &str) -> String {
    let label = crate::locale::tr("gui-stroke-out", &[]);
    body.replace("(stroke out)", &label)
}

// ── CMS implementation ──────────────────────────────────────────────────────

fn show_cms_maintenance(agent: &ureq::Agent, json: bool, discord: bool) -> Result<()> {
    if !json {
        println!("cmsdl {VERSION}: checking recent maintenance notice from region 'cms'.");
    }

    let news_list = fetch_cms_news_list(agent)?;
    let item = find_cms_maintenance(&news_list)?;

    let content = fetch_cms_news_content(agent, item.id)?;
    let mode = render_mode(json, discord);

    if json {
        let body = render_html(&content.content, mode);
        let body = collapse_blank_lines(&body);
        let ts =
            parse_cst_to_unix(&content.publish_date).context("failed to parse publish date")?;
        let output = MaintenanceOutput {
            id: item.id,
            title: content.title,
            publish_date: ts,
            body,
        };
        println!(
            "{}",
            serde_json::to_string(&output).unwrap_or_else(|_| "{}".into())
        );
    } else {
        eprintln!(
            "Found maintenance notice: #{} — {}",
            item.id, item.title
        );
        println!("=== {} ===", content.title);
        println!("Published: {}", content.publish_date);
        println!();
        let body = render_html(&content.content, mode);
        let compact = collapse_blank_lines(&body);
        if !compact.is_empty() {
            println!("{compact}");
        }
    }

    Ok(())
}

fn fetch_cms_news_list(agent: &ureq::Agent) -> Result<Vec<NewsCategory>> {
    let resp = agent
        .get(CMS_NEWS_LIST_URL)
        .call()
        .context("failed to fetch news list")?;
    let body = resp
        .into_string()
        .context("failed to read news list response")?;
    serde_json::from_str::<Vec<NewsCategory>>(&body).context("failed to parse news list JSON")
}

fn find_cms_maintenance(news_list: &[NewsCategory]) -> Result<&NewsItem> {
    let mut best: Option<&NewsItem> = None;

    for category in news_list {
        for item in &category.data {
            if item.title.contains("维护") || item.title.contains("停服公告") {
                match best {
                    None => best = Some(item),
                    Some(ref current) if item.id > current.id => best = Some(item),
                    _ => {}
                }
            }
        }
    }

    best.ok_or_else(|| anyhow::anyhow!("no maintenance announcement found"))
}

fn fetch_cms_news_content(agent: &ureq::Agent, id: u64) -> Result<NewsContentData> {
    let url = format!("{CMS_NEWS_CONTENT_URL}{id}");
    let resp = agent
        .get(&url)
        .call()
        .context("failed to fetch news content")?;
    let body = resp
        .into_string()
        .context("failed to read news content response")?;
    let response: NewsContentResponse =
        serde_json::from_str(&body).context("failed to parse news content JSON")?;

    if response.result != 0 {
        bail!(
            "API returned error: {} (code {})",
            response.message,
            response.result
        );
    }

    Ok(response.data)
}

// ── TMS implementation ──────────────────────────────────────────────────────

fn show_tms_maintenance(agent: &ureq::Agent, json: bool, discord: bool) -> Result<()> {
    if !json {
        println!("cmsdl {VERSION}: checking recent maintenance notice from region 'tms'.");
    }

    // 1. Obtain CSRF token from the TMS main page; ureq stores the
    //    accompanying antiforgery cookie automatically.
    let csrf_token = acquire_tms_csrf(agent)?;

    // 2. Fetch the first few pages of bulletins to find maintenance.
    let items = fetch_tms_bulletins(agent, &csrf_token, 5)?;

    // 3. Find the most recent 維護公告.
    let main_item = items
        .iter()
        .filter(|i| i.title.contains("維護公告"))
        .max_by_key(|i| &i.start_date)
        .ok_or_else(|| anyhow::anyhow!("no maintenance announcement (維護公告) found"))?;

    // 4. Fetch the full content of the main notice.
    let detail = fetch_tms_bulletin_detail(agent, &csrf_token, &main_item.bulletin_id)?;

    let mode = render_mode(json, discord);
    let mut body = render_html(&detail.content, mode);

    // 5. Check for a later 延後開機公告.
    let delayed = items.iter().find(|i| {
        i.title.contains("延後開機公告") && i.start_date >= main_item.start_date
    });

    let combined_title = detail.title.clone();
    if let Some(d) = delayed {
        if !json {
            eprintln!(
                "Also found delayed-opening notice: #{} — {}",
                d.bulletin_id, d.title
            );
        }
        if let Ok(dd) = fetch_tms_bulletin_detail(agent, &csrf_token, &d.bulletin_id) {
            body.push_str("\n\n---\n\n");
            if mode != RenderMode::Markdown {
                body.push_str(&format!("=== {} ===\n\n", dd.title));
            } else {
                body.push_str(&format!("### {}\n\n", dd.title));
            }
            body.push_str(&render_html(&dd.content, mode));
        }
    }

    let body = collapse_blank_lines(&body);
    let ts = parse_tms_date_to_unix(&detail.start_date)
        .context("failed to parse TMS start date")?;

    if json {
        let output = MaintenanceOutput {
            id: detail.bulletin_id.parse().unwrap_or(0),
            title: combined_title,
            publish_date: ts,
            body,
        };
        println!(
            "{}",
            serde_json::to_string(&output).unwrap_or_else(|_| "{}".into())
        );
    } else {
        eprintln!(
            "Found maintenance notice: #{} — {}",
            main_item.bulletin_id, main_item.title
        );
        println!("=== {} ===", detail.title);
        println!("Published: {}", detail.start_date);
        println!();
        if !body.is_empty() {
            println!("{body}");
        }
    }

    Ok(())
}

/// Obtain a fresh CSRF token from the TMS main page.
/// The antiforgery cookie is stored automatically by ureq's cookie jar.
fn acquire_tms_csrf(agent: &ureq::Agent) -> Result<String> {
    let resp = agent
        .get(TMS_MAIN_URL)
        .call()
        .context("failed to fetch TMS main page for CSRF")?;

    let body = resp
        .into_string()
        .context("failed to read TMS main page body")?;
    extract_csrf_from_html(&body)
}

/// Extract the `__RequestVerificationToken` value from TMS page HTML.
fn extract_csrf_from_html(html: &str) -> Result<String> {
    // Look for <input name="__RequestVerificationToken" ... value="..." />
    let needle = r#"name="__RequestVerificationToken""#;
    let pos = html
        .find(needle)
        .context("no __RequestVerificationToken in TMS page")?;
    let after = &html[pos..];
    let value_start = after
        .find("value=\"")
        .map(|p| p + 7)
        .context("no value attribute on CSRF input")?;
    let after_value = &after[value_start..];
    let value_end = after_value.find('"').unwrap_or(after_value.len());
    let token = &after_value[..value_end];

    if token.is_empty() {
        bail!("empty CSRF token");
    }
    Ok(token.to_string())
}

/// Fetch the first `pages` pages of TMS bulletins and return a combined list.
fn fetch_tms_bulletins(
    agent: &ureq::Agent,
    csrf_token: &str,
    pages: usize,
) -> Result<Vec<TmsBulletinItem>> {
    let mut all_items = Vec::new();

    for page in 1..=pages {
        let form = format!("Kind=0&Page={page}&method=0&PageSize=10");
        let resp = agent
            .post(TMS_BULLETIN_LIST_URL)
            .set("Content-Type", "application/x-www-form-urlencoded")
            .set("X-CSRF-Token", csrf_token)
            .send_string(&form)
            .context("failed to fetch TMS bulletin list")?;

        let body = resp
            .into_string()
            .context("failed to read TMS bulletin list response")?;

        let tms_resp: TmsResponse =
            serde_json::from_str(&body).context("failed to parse TMS bulletin list JSON")?;

        let items: Vec<TmsBulletinItem> =
            serde_json::from_value(tms_resp.data.my_data_set.table)
                .context("failed to parse TMS bulletin items")?;

        if items.is_empty() {
            break; // no more pages
        }
        all_items.extend(items);
    }

    Ok(all_items)
}

/// Fetch a single TMS bulletin's detail.
fn fetch_tms_bulletin_detail(
    agent: &ureq::Agent,
    csrf_token: &str,
    bid: &str,
) -> Result<TmsBulletinDetail> {
    let form = format!("Bid={bid}");
    let resp = agent
        .post(TMS_BULLETIN_DETAIL_URL)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .set("X-CSRF-Token", csrf_token)
        .send_string(&form)
        .context("failed to fetch TMS bulletin detail")?;

    let body = resp
        .into_string()
        .context("failed to read TMS bulletin detail response")?;

    let tms_resp: TmsResponse =
        serde_json::from_str(&body).context("failed to parse TMS bulletin detail JSON")?;

    let detail: TmsBulletinDetail = serde_json::from_value(tms_resp.data.my_data_set.table)
        .context("failed to parse TMS bulletin detail record")?;

    Ok(detail)
}

/// Parse a TMS date string like `2026/07/23` (UTC+8, date only) to Unix ts.
fn parse_tms_date_to_unix(s: &str) -> Result<i64> {
    let naive = chrono::NaiveDate::parse_from_str(s, "%Y/%m/%d")
        .context("invalid TMS date format")?;
    let naive_dt = naive
        .and_hms_opt(0, 0, 0)
        .context("invalid time")?;
    let cst = chrono::FixedOffset::east_opt(8 * 3600).context("invalid offset")?;
    let dt = naive_dt
        .and_local_timezone(cst)
        .single()
        .context("ambiguous local time")?;
    Ok(dt.timestamp())
}

/// Render HTML content to plain text, applying formatting according to the
/// requested [`RenderMode`].
fn render_html(html: &str, mode: RenderMode) -> String {
    let mut output = String::new();
    let mut color_stack: Vec<Option<AnsiColor>> = Vec::new();
    let mut bold_stack: Vec<bool> = Vec::new();
    let mut strike_stack: Vec<bool> = Vec::new();
    let mut i = 0;
    let bytes = html.as_bytes();

    while i < bytes.len() {
        if bytes[i] == b'<' {
            let tag_start = i;
            let mut tag_end = i;
            while tag_end < bytes.len() && bytes[tag_end] != b'>' {
                tag_end += 1;
            }
            if tag_end >= bytes.len() {
                output.push_str(&html[tag_start..]);
                break;
            }
            let tag = &html[tag_start + 1..tag_end];
            i = tag_end + 1;

            // Closing tag?
            if let Some(inner) = tag.strip_prefix('/') {
                let name = inner.split_whitespace().next().unwrap_or(inner);
                match name {
                    "span" | "font" => {
                        if let Some(color) = color_stack.pop() {
                            apply_close(&mut output, &color, mode);
                        }
                    }
                    "strong" | "b" => {
                        bold_stack.pop();
                        if bold_stack.is_empty()
                            || !bold_stack.last().copied().unwrap_or(false)
                        {
                            apply_bold_close(&mut output, mode);
                        }
                    }
                    "p" | "div" | "br" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        output.push('\n');
                    }
                    "a" => {
                        if let Some(color) = color_stack.pop() {
                            apply_close(&mut output, &color, mode);
                        }
                    }
                    "s" | "strike" | "del" => {
                        strike_stack.pop();
                        output.push_str(" (stroke out)");
                    }
                    _ => {}
                }
                continue;
            }

            // Self-closing tags.
            if tag.ends_with('/') {
                let name = tag.trim_end_matches('/').trim();
                if name == "br" {
                    output.push('\n');
                } else if let Some(src) = extract_img_src(tag) {
                    output.push_str(&format_img(src, mode));
                }
                continue;
            }

            // Opening tag.
            let name = tag.split_whitespace().next().unwrap_or(tag);

            match name {
                "span" | "font" => {
                    let color = extract_color(tag);
                    color_stack.push(color);
                    if let Some(c) = color {
                        apply_open(&mut output, c, mode);
                    }
                }
                "strong" | "b" => {
                    bold_stack.push(true);
                    apply_bold_open(&mut output, mode);
                }
                "p" | "div" | "br" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    output.push('\n');
                }
                "img" => {
                    if let Some(src) = extract_img_src(tag) {
                        output.push_str(&format_img(src, mode));
                    }
                }
                "a" => {
                    let link_color = AnsiColor {
                        r: 0x00,
                        g: 0x66,
                        b: 0xcc,
                    };
                    color_stack.push(Some(link_color));
                    apply_open(&mut output, link_color, mode);
                }
                "s" | "strike" | "del" => {
                    strike_stack.push(true);
                }
                _ => {}
            }
        } else if bytes[i] == b'&' {
            let ent_start = i;
            let mut ent_end = i;
            while ent_end < bytes.len() && bytes[ent_end] != b';' {
                ent_end += 1;
            }
            if ent_end >= bytes.len() {
                output.push('&');
                i += 1;
                continue;
            }
            let entity = &html[ent_start..=ent_end];
            match entity {
                "&nbsp;" => output.push(' '),
                "&lt;" => output.push('<'),
                "&gt;" => output.push('>'),
                "&amp;" => output.push('&'),
                "&quot;" => output.push('"'),
                "&apos;" => output.push('\''),
                "&ldquo;" => output.push('\u{201C}'),
                "&rdquo;" => output.push('\u{201D}'),
                _ => output.push_str(entity),
            }
            i = ent_end + 1;
        } else {
            let ch = html[i..].chars().next().unwrap_or('?');
            output.push(ch);
            i += ch.len_utf8();
        }
    }

    while let Some(color) = color_stack.pop() {
        apply_close(&mut output, &color, mode);
    }
    if !bold_stack.is_empty() {
        apply_bold_close(&mut output, mode);
    }

    output
}

// ── Formatting helpers ──────────────────────────────────────────────────────

fn render_mode(json: bool, discord: bool) -> RenderMode {
    if json && discord {
        RenderMode::Discord
    } else if json {
        RenderMode::Markdown
    } else {
        RenderMode::Terminal
    }
}

fn apply_bold_open(output: &mut String, mode: RenderMode) {
    match mode {
        RenderMode::Terminal | RenderMode::Gui => output.push_str("\x1b[1m"),
        RenderMode::Markdown => output.push_str("**"),
        RenderMode::Discord => {
            output.push_str("\x1b[1m**");
        }
    }
}

fn apply_bold_close(output: &mut String, mode: RenderMode) {
    match mode {
        RenderMode::Terminal | RenderMode::Gui => output.push_str("\x1b[22m"),
        RenderMode::Markdown => output.push_str("**"),
        RenderMode::Discord => {
            output.push_str("**\x1b[22m");
        }
    }
}

fn apply_open(output: &mut String, color: AnsiColor, mode: RenderMode) {
    // Skip grayscale — invisible on most terminals.
    if color.r == color.g && color.g == color.b {
        return;
    }
    match mode {
        RenderMode::Terminal | RenderMode::Gui => output.push_str(&color.to_ansi_fg()),
        RenderMode::Markdown => { /* no-op */ }
        RenderMode::Discord => output.push_str(&color.to_ansi_fg()),
    }
}

fn apply_close(output: &mut String, _color: &Option<AnsiColor>, mode: RenderMode) {
    match mode {
        RenderMode::Terminal | RenderMode::Gui => output.push_str("\x1b[39m"),
        RenderMode::Markdown => { /* no-op */ }
        RenderMode::Discord => output.push_str("\x1b[39m"),
    }
}

fn format_img(src: &str, mode: RenderMode) -> String {
    match mode {
        RenderMode::Terminal | RenderMode::Gui => format!("Image: {src}"),
        RenderMode::Markdown => format!("![image]({src})"),
        RenderMode::Discord => format!("![image]({src})"),
    }
}

/// Collapse runs of blank lines into at most one empty line between paragraphs.
fn collapse_blank_lines(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut blank_run = 0usize;
    let mut at_start = true;

    for line in text.lines() {
        // Treat ANSI-only lines as blank too.
        let is_blank = strip_ansi(line).trim().is_empty();
        if is_blank {
            blank_run += 1;
        } else {
            if at_start {
                at_start = false;
            } else if blank_run > 0 {
                // Emit at most one blank line between paragraphs.
                result.push('\n');
            }
            blank_run = 0;
            result.push_str(line);
            result.push('\n');
        }
    }
    // Trim trailing newline.
    while result.ends_with('\n') {
        result.pop();
    }
    result
}

/// Extract a color from a `<span style="color:#XXXXXX">` or similar tag.
fn extract_color(tag: &str) -> Option<AnsiColor> {
    // Look for style="color:#XXXXXX" or style="color: #XXXXXX"
    let style_attr = tag
        .split_whitespace()
        .find(|a| a.starts_with("style="))?;

    // Extract the value inside quotes.
    let style_value = style_attr
        .strip_prefix("style=")?
        .trim_matches('"')
        .trim_matches('\'');

    // Find the color: part.
    let color_part = style_value
        .split(';')
        .find(|p| p.trim().starts_with("color"))?;

    let hex = color_part
        .split(':')
        .nth(1)?
        .trim()
        .trim_start_matches('#');

    if hex.len() != 6 {
        return None;
    }

    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;

    // Skip grayscale colours (black, white, grey) — they carry no useful
    // formatting information and can render text invisible on some terminals.
    if r == g && g == b {
        return None;
    }

    Some(AnsiColor { r, g, b })
}

// ── HTML parsing helpers ────────────────────────────────────────────────────

/// Extract an attribute value from an HTML tag, e.g. `href="..."`.
fn extract_attr<'a>(tag: &'a str, attr_name: &str) -> Option<&'a str> {
    let prefix = format!("{attr_name}=");
    let attr = tag.split_whitespace().find(|a| a.starts_with(&prefix))?;
    let value = attr.strip_prefix(&prefix)?;
    Some(value.trim_matches('"').trim_matches('\''))
}

/// Extract the `src` attribute from an `<img>` tag.
fn extract_img_src(tag: &str) -> Option<&str> {
    extract_attr(tag, "src")
}

// ── Text post-processing ────────────────────────────────────────────────────

/// Parse a UTC+8 datetime string like `2026-07-24 17:15:29` into a Unix
/// timestamp (seconds since epoch, UTC).
fn parse_cst_to_unix(s: &str) -> Result<i64> {
    let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .context("invalid datetime format")?;
    let cst = chrono::FixedOffset::east_opt(8 * 3600)
        .context("invalid offset")?;
    let dt = naive
        .and_local_timezone(cst)
        .single()
        .context("ambiguous local time")?;
    Ok(dt.timestamp())
}

/// Strip ANSI escape sequences from a string (for blank-line detection).
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() == Some('[') {
                while let Some(n) = chars.next() {
                    if n == 'm' || n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A 24-bit RGB color, convertible to an ANSI escape sequence.
#[derive(Copy, Clone, Debug)]
struct AnsiColor {
    r: u8,
    g: u8,
    b: u8,
}

impl AnsiColor {
    /// Return the ANSI escape sequence to set the foreground to this color.
    fn to_ansi_fg(self) -> String {
        // Use 24-bit true color: \x1b[38;2;R;G;Bm
        format!("\x1b[38;2;{};{};{}m", self.r, self.g, self.b)
    }
}
