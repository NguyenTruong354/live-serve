//! The page shown for a folder without `index.html`.
//!
//! Built to stay light for big folders (`node_modules` has thousands of entries): the CSS, the
//! icons (CSS masks, defined once) and the script come once per page, and each row is a few
//! short tags with end tags omitted where HTML allows.

use std::fmt::Write;

/// Characters escaped in a file name used as one URL path segment.
const PATH_SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'/')
    .add(b'\\');

/// Appends `text` with `& < > "` escaped, without an intermediate string.
fn push_escaped(out: &mut String, text: &str) {
    let mut rest = text;
    while let Some(at) = rest.find(['&', '<', '>', '"']) {
        out.push_str(&rest[..at]);
        out.push_str(match rest.as_bytes()[at] {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            _ => "&quot;",
        });
        rest = &rest[at + 1..];
    }
    out.push_str(rest);
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    push_escaped(&mut out, text);
    out
}

/// One row of a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    /// Bytes; ignored for folders.
    pub size: u64,
    /// Last modified, as seconds since the Unix epoch.
    pub modified: Option<u64>,
}

/// "0 B", "999 B", "1.0 KB", "12.4 KB", "3.2 MB" (1 KB = 1024 B).
pub fn format_size(bytes: u64) -> String {
    let mut out = String::new();
    write_size(&mut out, bytes);
    out
}

fn write_size(out: &mut String, bytes: u64) {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        let _ = write!(out, "{bytes} B");
        return;
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let _ = write!(out, "{value:.1} {}", UNITS[unit]);
}

/// Days since 1970-01-01 → (year, month, day), proleptic Gregorian (Howard Hinnant's algorithm).
pub(crate) fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// Seconds since the Unix epoch → "YYYY-MM-DD HH:MM" in UTC.
pub fn format_timestamp(secs: u64) -> String {
    let mut out = String::new();
    write_timestamp(&mut out, secs);
    out
}

fn write_timestamp(out: &mut String, secs: u64) {
    let (y, m, d) = civil_from_days(secs / 86_400);
    let minutes = secs % 86_400 / 60;
    let _ = write!(out, "{y:04}-{m:02}-{d:02} {:02}:{:02}", minutes / 60, minutes % 60);
}

/// CSS class of the icon for a name: d (folder), h (html), c (code), s (style), i (image),
/// m (audio/video), j (data), t (text/docs), z (archive), f (anything else).
pub fn icon_class(name: &str, is_dir: bool) -> &'static str {
    if is_dir {
        return "d";
    }
    let ext = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
        _ => return "f",
    };
    match ext.as_str() {
        "html" | "htm" | "xhtml" => "h",
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "mts" | "cts" | "vue" | "svelte" | "rs" | "py"
        | "go" | "java" | "kt" | "c" | "h" | "cpp" | "hpp" | "cs" | "rb" | "php" | "sh" | "ps1"
        | "swift" | "wasm" => "c",
        "css" | "scss" | "sass" | "less" | "styl" => "s",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "svg" | "ico" | "bmp" => "i",
        "mp4" | "webm" | "mov" | "mkv" | "mp3" | "wav" | "ogg" | "m4a" | "flac" => "m",
        "json" | "jsonc" | "yaml" | "yml" | "toml" | "xml" | "csv" | "lock" | "map" | "ini" | "env" => "j",
        "md" | "mdx" | "txt" | "rst" | "pdf" | "log" => "t",
        "zip" | "gz" | "tgz" | "tar" | "7z" | "rar" | "bz2" | "xz" => "z",
        _ => "f",
    }
}

/// Styles, icons and the light/dark palette; once per page.
const STYLE: &str = r#"<style>
:root{color-scheme:light dark;--bg:#fff;--fg:#1f2328;--mute:#656d76;--line:#d8dee4;--hover:#f6f8fa;--link:#0969da;--field:#fff}
@media (prefers-color-scheme: dark){:root{--bg:#0d1117;--fg:#e6edf3;--mute:#8d96a0;--line:#30363d;--hover:#161b22;--link:#4493f8;--field:#010409}}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);font:14px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif}
header{position:sticky;top:0;z-index:1;display:flex;flex-wrap:wrap;gap:12px;align-items:center;justify-content:space-between;padding:16px 24px;background:var(--bg);border-bottom:1px solid var(--line)}
nav{font-size:18px;font-weight:600;word-break:break-all}
nav a{color:var(--fg);text-decoration:none}
nav a:hover{color:var(--link);text-decoration:underline}
nav span{color:var(--mute);margin:0 6px;font-weight:400}
input{width:min(280px,100%);padding:6px 10px;border:1px solid var(--line);border-radius:6px;background:var(--field);color:var(--fg);font:inherit}
input:focus{outline:2px solid var(--link);outline-offset:-1px}
main{padding:8px 24px 24px}
table{width:100%;border-collapse:collapse}
th{text-align:left;font-weight:600;color:var(--mute);font-size:12px;padding:8px;border-bottom:1px solid var(--line)}
td{padding:6px 8px;border-bottom:1px solid var(--line);white-space:nowrap}
tbody tr:hover{background:var(--hover)}
td:first-child{width:100%;white-space:normal;word-break:break-all}
th:nth-child(n+2),td:nth-child(n+2){color:var(--mute);text-align:right;font-variant-numeric:tabular-nums}
td a{display:inline-flex;align-items:center;gap:8px;color:var(--link);text-decoration:none}
td a:hover{text-decoration:underline}
td a::before{content:"";flex:none;width:18px;height:18px;background:var(--c,#8d96a0);-webkit-mask:var(--i) center/contain no-repeat;mask:var(--i) center/contain no-repeat}
a{--i:url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'%3E%3Cpath fill-rule='evenodd' d='M6 2h8l5 5v14a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1zm8 1.5V7h3.5z'/%3E%3C/svg%3E")}
.d{--c:#d4a72c;--i:url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'%3E%3Cpath d='M3 6.5A1.5 1.5 0 0 1 4.5 5h4.6l2 2.2h8.4A1.5 1.5 0 0 1 21 8.7v9.8a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 18.5z'/%3E%3C/svg%3E")}
.u{--i:url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'%3E%3Cpath d='M12 4l-7 7h4.5v9h5v-9H19z'/%3E%3C/svg%3E")}
.h{--c:#e34c26}.c{--c:#3178c6}.s{--c:#8250df}.i{--c:#1a7f37}.m{--c:#bf3989}.j{--c:#0e8a7d}.t{--c:#57606a}.z{--c:#9a6700}
footer{padding:0 24px 24px;color:var(--mute);font-size:12px}
footer a{color:inherit}
#none{padding:24px 8px;color:var(--mute)}
@media (max-width:600px){header,main,footer{padding-left:12px;padding-right:12px}th:nth-child(3),td:nth-child(3){display:none}}
</style>"#;

/// Filter box, "/" to focus it, Esc to clear, and dates shown in the viewer's local time.
const SCRIPT: &str = r#"<script>(()=>{const q=document.getElementById("q"),rows=[...document.querySelectorAll("tbody tr:not(.up)")],none=document.getElementById("none");
const f=new Intl.DateTimeFormat(undefined,{dateStyle:"medium",timeStyle:"short"});for(const t of document.getElementsByTagName("time")){const d=new Date(t.textContent.replace(" ","T")+"Z");if(!isNaN(d)){t.title=t.textContent+" UTC";t.textContent=f.format(d)}}
const names=rows.map(r=>r.cells[0].textContent.toLowerCase());
q.addEventListener("input",()=>{const v=q.value.trim().toLowerCase();let n=0;rows.forEach((r,i)=>{const show=names[i].includes(v);r.hidden=!show;if(show)n++});none.hidden=n>0;none.textContent=rows.length?"No matches":"This folder is empty"});
addEventListener("keydown",e=>{if(e.key==="/"&&document.activeElement!==q){e.preventDefault();q.focus()}else if(e.key==="Escape"&&document.activeElement===q){q.value="";q.dispatchEvent(new Event("input"));q.blur()}});
})();</script>"#;

/// The listing page for `url_path` (as requested, percent-encoded): folders first, then files,
/// each sorted case-insensitively.
pub fn directory_listing(url_path: &str, entries: &[Entry]) -> String {
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by_cached_key(|e| (!e.is_dir, e.name.to_lowercase()));

    let segments: Vec<&str> = url_path.split('/').filter(|s| !s.is_empty()).collect();
    let decoded: Vec<String> = segments
        .iter()
        .map(|s| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned())
        .collect();
    let mut shown_path = String::from("/");
    for name in &decoded {
        shown_path.push_str(name);
        shown_path.push('/');
    }
    let title = escape_html(&shown_path);

    let names: usize = sorted.iter().map(|e| e.name.len()).sum();
    let mut html = String::with_capacity(STYLE.len() + SCRIPT.len() + 1024 + names * 2 + sorted.len() * 80);
    let _ = write!(
        html,
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Index of {title}</title>{STYLE}</head><body>\
<header><nav aria-label=\"Path\"><a href=\"/\">~</a>"
    );
    let mut href = String::from("/");
    for (raw, name) in segments.iter().zip(&decoded) {
        href.push_str(raw);
        href.push('/');
        let _ = write!(html, "<span>/</span><a href=\"{}\">{}</a>", escape_html(&href), escape_html(name));
    }
    html.push_str(
        "</nav><input id=\"q\" type=\"search\" placeholder=\"Filter (press /)\" autocomplete=\"off\" \
spellcheck=\"false\" aria-label=\"Filter\"></header><main><table><thead><tr><th>Name<th>Size<th>Modified<tbody>\n",
    );

    if !segments.is_empty() {
        html.push_str("<tr class=up><td><a class=u href=\"../\">..</a><td><td>\n");
    }
    // Each row is written straight into `html`: no per-row strings. End tags of tr/td are optional.
    let (mut folders, mut files) = (0, 0);
    for entry in &sorted {
        let slash = if entry.is_dir { "/" } else { "" };
        let _ = write!(
            html,
            "<tr><td><a class={} href=\"{}{slash}\">",
            icon_class(&entry.name, entry.is_dir),
            percent_encoding::utf8_percent_encode(&entry.name, PATH_SEGMENT)
        );
        push_escaped(&mut html, &entry.name);
        html.push_str(slash);
        html.push_str("</a><td>");
        if entry.is_dir {
            folders += 1;
        } else {
            files += 1;
            write_size(&mut html, entry.size);
        }
        html.push_str("<td>");
        if let Some(secs) = entry.modified {
            html.push_str("<time>");
            write_timestamp(&mut html, secs);
            html.push_str("</time>");
        }
        html.push('\n');
    }

    let empty = sorted.is_empty();
    let _ = write!(
        html,
        "</table><p id=\"none\"{}>{}</p></main><footer>{folders} folder{}, {files} file{} · served by live-serve</footer>{SCRIPT}</body></html>\n",
        if empty { "" } else { " hidden" },
        if empty { "This folder is empty" } else { "No matches" },
        if folders == 1 { "" } else { "s" },
        if files == 1 { "" } else { "s" },
    );
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: u64) -> Entry {
        Entry { name: name.into(), is_dir: false, size, modified: Some(1_792_369_800) }
    }

    fn dir(name: &str) -> Entry {
        Entry { name: name.into(), is_dir: true, size: 0, modified: None }
    }

    #[test]
    fn sizes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(12_697), "12.4 KB");
        assert_eq!(format_size(3_355_443), "3.2 MB");
        assert_eq!(format_size(1_610_612_736), "1.5 GB");
    }

    #[test]
    fn timestamps_in_utc() {
        assert_eq!(format_timestamp(0), "1970-01-01 00:00");
        assert_eq!(format_timestamp(1_792_369_800), "2026-10-19 00:30");
        assert_eq!(format_timestamp(951_782_400), "2000-02-29 00:00", "leap day");
    }

    #[test]
    fn icons_by_kind() {
        assert_eq!(icon_class("src", true), "d");
        assert_eq!(icon_class("index.HTML", false), "h");
        assert_eq!(icon_class("app.tsx", false), "c");
        assert_eq!(icon_class("main.rs", false), "c");
        assert_eq!(icon_class("site.scss", false), "s");
        assert_eq!(icon_class("logo.svg", false), "i");
        assert_eq!(icon_class("clip.mp4", false), "m");
        assert_eq!(icon_class("package.json", false), "j");
        assert_eq!(icon_class("README.md", false), "t");
        assert_eq!(icon_class("dist.tar.gz", false), "z");
        assert_eq!(icon_class("LICENSE", false), "f");
        assert_eq!(icon_class(".gitignore", false), "f");
    }

    #[test]
    fn breadcrumb_links_every_level() {
        let html = directory_listing("/docs/guide%20s/", &[]);
        assert!(html.contains(r#"<a href="/">"#), "root");
        assert!(html.contains(r#"<a href="/docs/">docs</a>"#));
        assert!(html.contains("guide s"), "shown decoded");
        assert!(html.contains("<title>Index of /docs/guide s/</title>"));
    }

    #[test]
    fn rows_have_icon_size_and_date() {
        let html = directory_listing("/", &[file("app.js", 12_697), dir("src")]);
        assert!(html.contains(r#"<a class=c href="app.js">app.js</a>"#), "{html}");
        assert!(html.contains("<td>12.4 KB<td><time>2026-10-19 00:30</time>"));
        assert!(html.contains(r#"<a class=d href="src/">src/</a><td><td>"#));
        assert!(html.contains("1 folder, 1 file"));
    }

    /// Big folders stay light: a row costs its name twice plus a fixed ~80 bytes.
    #[test]
    fn rows_stay_small() {
        let entries: Vec<Entry> = (0..1000).map(|i| file(&format!("package-name-{i:04}.js"), 4096)).collect();
        let html = directory_listing("/node_modules/", &entries);
        let names: usize = entries.iter().map(|e| e.name.len() * 2).sum();
        let budget = directory_listing("/node_modules/", &[]).len() + names + entries.len() * 80;
        assert!(html.len() < budget, "{} bytes, budget {budget}", html.len());
    }

    #[test]
    fn folders_first_then_case_insensitive() {
        let html = directory_listing("/", &[file("b.txt", 1), file("A.txt", 1), dir("zeta"), dir("Alpha")]);
        let at = |s: &str| html.find(s).unwrap();
        assert!(at("Alpha/") < at("zeta/"));
        assert!(at("zeta/") < at("A.txt"));
        assert!(at("A.txt") < at("b.txt"));
    }

    #[test]
    fn parent_link_except_at_the_root() {
        assert!(directory_listing("/docs/", &[]).contains(r#"href="../""#));
        assert!(!directory_listing("/", &[]).contains(r#"href="../""#));
    }

    #[test]
    fn names_and_paths_are_escaped() {
        let html = directory_listing("/a%3Cb%3E/", &[file("<x>.html", 1), file("a\"b.txt", 1)]);
        assert!(!html.contains("<x>"));
        assert!(!html.contains("<b>"));
        assert!(html.contains("&lt;x&gt;.html"));
        assert!(html.contains("a&quot;b.txt"));
        assert!(html.contains(r#"href="%3Cx%3E.html""#));
    }

    #[test]
    fn column_styles_do_not_clash_with_icon_classes() {
        let html = directory_listing("/", &[file("a.css", 1), file("b.md", 1)]);
        assert!(!html.contains("<td class"), "columns are styled by position, not class");
        for icon in ["d", "h", "c", "s", "i", "m", "j", "t", "z", "f"] {
            assert!(!html.contains(&format!(".{icon},")), "no shared rule for .{icon}");
        }
    }

    #[test]
    fn has_filter_and_dark_mode() {
        let html = directory_listing("/", &[]);
        assert!(html.contains(r#"type="search""#));
        assert!(html.contains("prefers-color-scheme: dark"));
        assert!(html.contains("<script>"));
    }
}
