//! ModWorkshop and Nexus Mods API clients (blocking; called from worker threads).

use crate::model::{Dependency, FileKind, RemoteFile, RemoteMod, Source, TagChip};
use crate::nsfw;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;

pub const APP_NAME: &str = "PD2ModManager";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn client() -> &'static reqwest::blocking::Client {
    static C: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .user_agent(format!("{APP_NAME}/{APP_VERSION}"))
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60 * 30))
            .build()
            .expect("http client")
    })
}

fn get_json(url: &str) -> Result<Value> {
    let resp = client().get(url).header("Accept", "application/json").timeout(Duration::from_secs(30)).send()?;
    let status = resp.status();
    if !status.is_success() {
        bail!("{} returned HTTP {}", url, status);
    }
    Ok(resp.json()?)
}

fn s(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn u(v: &Value) -> u64 {
    match v {
        Value::Number(n) => n.as_u64().unwrap_or(0),
        Value::String(s) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn parse_hex(c: &str) -> Option<[u8; 3]> {
    let c = c.trim_start_matches('#');
    if c.len() != 6 {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&c[i..i + 2], 16).ok();
    Some([p(0)?, p(2)?, p(4)?])
}

/// Turn a requirement link into something installable when it points at ModWorkshop or Nexus.
pub fn dep_from_url(name: &str, url: &str, optional: bool) -> Dependency {
    let id_after = |marker: &str| -> Option<u64> {
        let rest = url.split(marker).nth(1)?;
        rest.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().ok()
    };
    let (source, id) = if let Some(id) = id_after("modworkshop.net/mod/") {
        (Source::ModWorkshop, Some(id))
    } else if let Some(id) = id_after("nexusmods.com/payday2/mods/") {
        (Source::Nexus, Some(id))
    } else {
        (Source::Local, None)
    };
    Dependency {
        source,
        id,
        name: name.trim().to_string(),
        url: (!url.trim().is_empty()).then(|| url.trim().to_string()),
        optional,
    }
}

// ======================= ModWorkshop =======================

pub mod mws {
    use super::*;
    pub const API: &str = "https://api.modworkshop.net";
    pub const GAME_ID: u64 = 1;
    const IMG: &str = "https://storage.modworkshop.net/mods/images/";

    #[derive(Debug, Clone)]
    pub struct Category {
        pub id: u64,
        pub name: String,
        pub parent: Option<u64>,
    }

    #[derive(Debug, Clone)]
    pub struct Query {
        pub text: String,
        pub sort: &'static str,
        pub tags: Vec<u64>,
        pub category: Option<u64>,
        pub page: u32,
    }

    pub const SORTS: &[(&str, &str)] = &[
        ("bumped_at", "Recently updated"),
        ("published_at", "Newest"),
        ("downloads", "Most downloaded"),
        ("likes", "Most liked"),
        ("weekly_score", "Popular this week"),
        ("score", "Popular all time"),
        ("views", "Most viewed"),
        ("name", "Name"),
    ];

    pub fn parse_mod(v: &Value) -> RemoteMod {
        let tags: Vec<TagChip> = v["tags"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|t| TagChip {
                        id: Some(u(&t["id"])),
                        name: s(&t["name"]),
                        color: parse_hex(&s(&t["color"])),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let thumb_file = s(&v["thumbnail"]["file"]);
        let (thumb, image) = if thumb_file.is_empty() {
            (None, None)
        } else {
            let full = format!("{IMG}{thumb_file}");
            let t = if v["thumbnail"]["has_thumb"].as_bool().unwrap_or(false) {
                format!("{IMG}thumbnail_{thumb_file}")
            } else {
                full.clone()
            };
            (Some(t), Some(full))
        };
        let name = s(&v["name"]);
        let summary = s(&v["short_desc"]);
        let description = s(&v["desc"]);
        let category = s(&v["category"]["name"]);
        let mut tag_names: Vec<String> = tags.iter().map(|t| t.name.clone()).collect();
        tag_names.push(category.clone());
        let nsfw = nsfw::mod_is_nsfw(&name, &summary, &description, &tag_names);
        let id = u(&v["id"]);
        let version = {
            let cv = s(&v["custom_version"]);
            if cv.is_empty() { s(&v["version"]) } else { cv }
        };
        RemoteMod {
            source: Source::ModWorkshop,
            id,
            name,
            author: s(&v["user"]["name"]),
            summary,
            description,
            thumb,
            image,
            tags,
            category,
            downloads: u(&v["downloads"]),
            likes: u(&v["likes"]),
            version,
            updated: s(&v["bumped_at"]).chars().take(10).collect(),
            nsfw,
            page_url: format!("https://modworkshop.net/mod/{id}"),
        }
    }

    /// Returns (mods, has_more).
    pub fn search(q: &Query) -> Result<(Vec<RemoteMod>, bool)> {
        let mut url = format!("{API}/games/{GAME_ID}/mods?limit=30&page={}&sort={}", q.page, q.sort);
        if !q.text.trim().is_empty() {
            url.push_str(&format!("&query={}", urlencoding::encode(q.text.trim())));
            if q.sort == "bumped_at" {
                url = url.replace("sort=bumped_at", "sort=best_match");
            }
        }
        for t in &q.tags {
            url.push_str(&format!("&tags[]={t}"));
        }
        if let Some(c) = q.category {
            url.push_str(&format!("&category_id={c}"));
        }
        let v = get_json(&url)?;
        let mods = v["data"].as_array().map(|a| a.iter().map(parse_mod).collect()).unwrap_or_default();
        let cur = u(&v["meta"]["current_page"]);
        let last = u(&v["meta"]["last_page"]);
        Ok((mods, cur < last))
    }

    pub fn tags() -> Result<Vec<TagChip>> {
        let v = get_json(&format!("{API}/games/{GAME_ID}/tags?global=1&limit=50"))?;
        let mut out: Vec<TagChip> = v["data"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|t| TagChip { id: Some(u(&t["id"])), name: s(&t["name"]), color: parse_hex(&s(&t["color"])) })
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by_key(|a| a.name.to_lowercase());
        Ok(out)
    }

    pub fn categories() -> Result<Vec<Category>> {
        let mut out = vec![];
        let mut page = 1;
        loop {
            let v = get_json(&format!("{API}/games/{GAME_ID}/categories?limit=50&page={page}"))?;
            if let Some(a) = v["data"].as_array() {
                for c in a {
                    out.push(Category {
                        id: u(&c["id"]),
                        name: s(&c["name"]),
                        parent: c["parent_id"].as_u64(),
                    });
                }
            }
            let last = u(&v["meta"]["last_page"]).max(1);
            if page as u64 >= last || page > 20 {
                break;
            }
            page += 1;
        }
        out.sort_by_key(|a| a.name.to_lowercase());
        Ok(out)
    }

    pub fn detail(id: u64) -> Result<(RemoteMod, bool)> {
        let v = get_json(&format!("{API}/mods/{id}"))?;
        let disable_managers = v["disable_mod_managers"].as_bool().unwrap_or(false);
        Ok((parse_mod(&v), disable_managers))
    }

    /// Install-time info: does the author forbid mod managers, which download is the
    /// main one ("file"/"link", id), and the mod's current version.
    pub struct Primary {
        pub managers_disabled: bool,
        pub download: Option<(String, u64)>,
        pub version: String,
        pub deps: Vec<Dependency>,
    }

    /// Dependencies listed on the mod plus those from its instructions template
    /// (that's where most "needs BeardLib / SuperBLT" requirements live).
    pub fn parse_deps(v: &Value) -> Vec<Dependency> {
        let self_id = u(&v["id"]);
        let mut out: Vec<Dependency> = vec![];
        let lists = [&v["dependencies"], &v["instructs_template"]["dependencies"]];
        for list in lists {
            let Some(arr) = list.as_array() else { continue };
            for d in arr {
                let optional = d["optional"].as_bool().unwrap_or(false);
                let mod_id = u(&d["mod_id"]);
                let dep = if mod_id > 0 {
                    let name = {
                        let n = s(&d["mod"]["name"]);
                        if n.is_empty() { s(&d["name"]) } else { n }
                    };
                    Dependency {
                        source: Source::ModWorkshop,
                        id: Some(mod_id),
                        name: if name.is_empty() { format!("ModWorkshop mod {mod_id}") } else { name },
                        url: Some(format!("https://modworkshop.net/mod/{mod_id}")),
                        optional,
                    }
                } else {
                    let name = s(&d["name"]);
                    let url = s(&d["url"]);
                    if name.is_empty() && url.is_empty() {
                        continue;
                    }
                    dep_from_url(if name.is_empty() { &url } else { &name }, &url, optional)
                };
                if dep.id == Some(self_id) && dep.source == Source::ModWorkshop {
                    continue;
                }
                let dup = out.iter_mut().find(|o| {
                    (o.id.is_some() && o.id == dep.id && o.source == dep.source) || o.name.eq_ignore_ascii_case(&dep.name)
                });
                match dup {
                    // Required wins over optional if listed twice.
                    Some(o) => o.optional &= dep.optional,
                    None => out.push(dep),
                }
            }
        }
        out
    }

    pub fn primary(id: u64) -> Result<Primary> {
        let v = get_json(&format!("{API}/mods/{id}"))?;
        let m = parse_mod(&v);
        let kind = s(&v["download_type"]);
        let dl = u(&v["download_id"]);
        Ok(Primary {
            managers_disabled: v["disable_mod_managers"].as_bool().unwrap_or(false),
            download: (!kind.is_empty() && dl > 0).then_some((kind, dl)),
            version: m.version,
            deps: parse_deps(&v),
        })
    }

    pub fn files(id: u64) -> Result<Vec<RemoteFile>> {
        let mut out = vec![];
        let v = get_json(&format!("{API}/mods/{id}/files?limit=50"))?;
        if let Some(a) = v["data"].as_array() {
            for f in a {
                let url = s(&f["download_url"]);
                let fid = u(&f["id"]);
                out.push(RemoteFile {
                    id: fid,
                    name: s(&f["name"]),
                    version: s(&f["version"]),
                    size: u(&f["size"]),
                    label: s(&f["label"]),
                    kind: FileKind::Direct(if url.is_empty() {
                        format!("{API}/files/{fid}/download")
                    } else {
                        url
                    }),
                });
            }
        }
        if let Ok(v) = get_json(&format!("{API}/mods/{id}/links?limit=50")) {
            if let Some(a) = v["data"].as_array() {
                for l in a {
                    out.push(RemoteFile {
                        id: u(&l["id"]),
                        name: s(&l["name"]),
                        version: s(&l["version"]),
                        size: 0,
                        label: {
                            let lb = s(&l["label"]);
                            if lb.is_empty() { "External link".into() } else { lb }
                        },
                        kind: FileKind::Link(s(&l["url"])),
                    });
                }
            }
        }
        Ok(out)
    }

    pub fn version(id: u64) -> Result<String> {
        let (m, _) = detail(id)?;
        Ok(m.version)
    }
}

// ======================= Nexus Mods =======================

pub mod nexus {
    use super::*;
    pub const GQL: &str = "https://api.nexusmods.com/v2/graphql";
    pub const V1: &str = "https://api.nexusmods.com/v1";
    pub const DOMAIN: &str = "payday2";
    pub const GAME_ID: u64 = 648;

    pub const SORTS: &[(&str, &str)] = &[
        ("updatedAt", "Recently updated"),
        ("createdAt", "Newest"),
        ("downloads", "Most downloaded"),
        ("endorsements", "Most endorsed"),
        ("name", "Name"),
    ];

    #[derive(Debug, Clone)]
    pub struct Query {
        pub text: String,
        pub sort: &'static str,
        pub category: Option<String>,
        pub hide_adult: bool,
        pub offset: u32,
    }

    fn gql(query: &str) -> Result<Value> {
        let resp = client()
            .post(GQL)
            .timeout(Duration::from_secs(30))
            .header("Application-Name", APP_NAME)
            .header("Application-Version", APP_VERSION)
            .json(&json!({ "query": query }))
            .send()?;
        let status = resp.status();
        let v: Value = resp.json().with_context(|| format!("Nexus HTTP {status}"))?;
        if let Some(errs) = v["errors"].as_array() {
            if !errs.is_empty() && v["data"].is_null() {
                bail!("Nexus API error: {}", s(&errs[0]["message"]));
            }
        }
        Ok(v)
    }

    fn lit(text: &str) -> String {
        serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into())
    }

    fn parse_mod(n: &Value) -> RemoteMod {
        let id = u(&n["modId"]);
        let name = s(&n["name"]);
        let summary = html_to_text(&s(&n["summary"]));
        let description = html_to_text(&s(&n["description"]));
        let category = s(&n["modCategory"]["name"]);
        let adult = n["adultContent"].as_bool().unwrap_or(false);
        let nsfw = adult || nsfw::mod_is_nsfw(&name, &summary, "", std::slice::from_ref(&category));
        let tags = if category.is_empty() {
            vec![]
        } else {
            vec![TagChip { id: None, name: category.clone(), color: None }]
        };
        RemoteMod {
            source: Source::Nexus,
            id,
            name,
            author: s(&n["author"]),
            summary,
            description,
            thumb: n["thumbnailUrl"].as_str().map(String::from),
            image: n["pictureUrl"].as_str().map(String::from),
            tags,
            category,
            downloads: u(&n["downloads"]),
            likes: u(&n["endorsements"]),
            version: s(&n["version"]),
            updated: s(&n["updatedAt"]).chars().take(10).collect(),
            nsfw,
            page_url: format!("https://www.nexusmods.com/{DOMAIN}/mods/{id}"),
        }
    }

    const FIELDS: &str = "modId name summary author pictureUrl thumbnailUrl adultContent endorsements downloads version updatedAt modCategory { name }";

    pub fn search(q: &Query) -> Result<(Vec<RemoteMod>, bool)> {
        let mut filter = format!("gameDomainName: [{{value: {}, op: EQUALS}}]", lit(DOMAIN));
        if !q.text.trim().is_empty() {
            filter.push_str(&format!(", name: [{{value: {}, op: WILDCARD}}]", lit(q.text.trim())));
        }
        if let Some(c) = &q.category {
            filter.push_str(&format!(", categoryName: [{{value: {}, op: EQUALS}}]", lit(c)));
        }
        if q.hide_adult {
            filter.push_str(", adultContent: [{value: false, op: EQUALS}]");
        }
        let dir = if q.sort == "name" { "ASC" } else { "DESC" };
        let query = format!(
            "query {{ mods(filter: {{ {filter} }}, count: 30, offset: {}, sort: [{{{}: {{direction: {dir}}}}}]) {{ totalCount nodes {{ {FIELDS} }} }} }}",
            q.offset, q.sort
        );
        let v = gql(&query)?;
        let total = u(&v["data"]["mods"]["totalCount"]);
        let mods: Vec<RemoteMod> = v["data"]["mods"]["nodes"]
            .as_array()
            .map(|a| a.iter().map(parse_mod).collect())
            .unwrap_or_default();
        let has_more = (q.offset as u64 + mods.len() as u64) < total;
        Ok((mods, has_more))
    }

    pub fn detail(id: u64) -> Result<RemoteMod> {
        let query = format!(
            "query {{ mods(filter: {{ gameId: [{{value: \"{GAME_ID}\", op: EQUALS}}], modId: [{{value: \"{id}\", op: EQUALS}}] }}, count: 1) {{ nodes {{ {FIELDS} description }} }} }}"
        );
        let v = gql(&query)?;
        let n = v["data"]["mods"]["nodes"].get(0).ok_or_else(|| anyhow!("Mod {id} not found on Nexus"))?;
        Ok(parse_mod(n))
    }

    pub fn files(mod_id: u64) -> Result<Vec<RemoteFile>> {
        let query = format!(
            "query {{ modFiles(modId: {mod_id}, gameId: {GAME_ID}) {{ fileId name version sizeInBytes category primary }} }}"
        );
        let v = gql(&query)?;
        let mut out = vec![];
        if let Some(a) = v["data"]["modFiles"].as_array() {
            for f in a {
                let cat = s(&f["category"]);
                if matches!(cat.as_str(), "ARCHIVED" | "DELETED" | "OLD_VERSION" | "REMOVED") {
                    continue;
                }
                out.push(RemoteFile {
                    id: u(&f["fileId"]),
                    name: s(&f["name"]),
                    version: s(&f["version"]),
                    size: u(&f["sizeInBytes"]),
                    label: cat.to_lowercase().replace('_', " "),
                    kind: FileKind::Nexus { mod_id, file_id: u(&f["fileId"]) },
                });
            }
        }
        // MAIN files first
        out.sort_by_key(|f| if f.label == "main" { 0 } else { 1 });
        Ok(out)
    }

    pub struct NexusUser {
        pub name: String,
        pub premium: bool,
    }

    pub fn validate(api_key: &str) -> Result<NexusUser> {
        let resp = client()
            .get(format!("{V1}/users/validate.json"))
            .header("apikey", api_key.trim())
            .header("Application-Name", APP_NAME)
            .header("Application-Version", APP_VERSION)
            .send()?;
        if !resp.status().is_success() {
            bail!("API key rejected (HTTP {})", resp.status());
        }
        let v: Value = resp.json()?;
        Ok(NexusUser { name: s(&v["name"]), premium: v["is_premium"].as_bool().unwrap_or(false) })
    }

    /// Ask Nexus for a CDN link. Without key/expires this only works for Premium accounts.
    pub fn download_link(api_key: &str, mod_id: u64, file_id: u64, nxm_key: Option<(&str, &str)>) -> Result<String> {
        if api_key.trim().is_empty() {
            bail!("NEEDS_KEY");
        }
        let mut url = format!("{V1}/games/{DOMAIN}/mods/{mod_id}/files/{file_id}/download_link.json");
        if let Some((k, e)) = nxm_key {
            url.push_str(&format!("?key={}&expires={}", urlencoding::encode(k), urlencoding::encode(e)));
        }
        let resp = client()
            .get(&url)
            .header("apikey", api_key.trim())
            .header("Application-Name", APP_NAME)
            .header("Application-Version", APP_VERSION)
            .send()?;
        let status = resp.status();
        if status.as_u16() == 403 && nxm_key.is_none() {
            bail!("NEEDS_PREMIUM");
        }
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            bail!("Nexus refused the download (HTTP {status}): {}", body.chars().take(200).collect::<String>());
        }
        let v: Value = resp.json()?;
        v.get(0)
            .and_then(|x| x["URI"].as_str())
            .map(String::from)
            .ok_or_else(|| anyhow!("Nexus returned no download link"))
    }

    /// Parsed `nxm://payday2/mods/<mod>/files/<file>?key=..&expires=..`
    #[derive(Debug, Clone)]
    pub struct NxmLink {
        pub mod_id: u64,
        pub file_id: u64,
        pub key: String,
        pub expires: String,
    }

    pub fn parse_nxm(link: &str) -> Result<NxmLink> {
        let rest = link.trim().strip_prefix("nxm://").ok_or_else(|| anyhow!("Not an nxm:// link"))?;
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        let parts: Vec<&str> = path.trim_end_matches('/').split('/').collect();
        if parts.len() < 5 || !parts[0].eq_ignore_ascii_case(DOMAIN) {
            bail!("This nxm link is not for PAYDAY 2");
        }
        let mod_id = parts[2].parse()?;
        let file_id = parts[4].parse()?;
        let mut key = String::new();
        let mut expires = String::new();
        for kv in query.split('&') {
            if let Some((k, v)) = kv.split_once('=') {
                match k {
                    "key" => key = v.to_string(),
                    "expires" => expires = v.to_string(),
                    _ => {}
                }
            }
        }
        Ok(NxmLink { mod_id, file_id, key, expires })
    }

    pub fn version(id: u64) -> Result<String> {
        Ok(detail(id)?.version)
    }

    /// Mods this Nexus mod requires. Off-site requirements that link to ModWorkshop
    /// or Nexus pages become installable too.
    pub fn requirements(mod_id: u64) -> Result<Vec<Dependency>> {
        let query = format!(
            "query {{ mods(filter: {{ gameId: [{{value: \"{GAME_ID}\", op: EQUALS}}], modId: [{{value: \"{mod_id}\", op: EQUALS}}] }}, count: 1) {{ nodes {{ modRequirements {{ nexusRequirements {{ nodes {{ modId modName gameId url notes }} }} }} }} }} }}"
        );
        let v = gql(&query)?;
        let mut out: Vec<Dependency> = vec![];
        let nodes = &v["data"]["mods"]["nodes"][0]["modRequirements"]["nexusRequirements"]["nodes"];
        for n in nodes.as_array().cloned().unwrap_or_default() {
            let name = s(&n["modName"]);
            let url = s(&n["url"]);
            let optional = s(&n["notes"]).to_lowercase().contains("optional");
            let rid = u(&n["modId"]);
            let dep = if rid > 0 && u(&n["gameId"]) == GAME_ID {
                Dependency {
                    source: Source::Nexus,
                    id: Some(rid),
                    name,
                    url: Some(format!("https://www.nexusmods.com/{DOMAIN}/mods/{rid}")),
                    optional,
                }
            } else {
                dep_from_url(&name, &url, optional)
            };
            if dep.source == Source::Nexus && dep.id == Some(mod_id) {
                continue;
            }
            if !out.iter().any(|o| o.name.eq_ignore_ascii_case(&dep.name)) {
                out.push(dep);
            }
        }
        Ok(out)
    }
}

/// Very small HTML/BBCode → plain-text cleanup for descriptions.
pub fn html_to_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    let replaced = s
        .replace("<br />", "\n")
        .replace("<br/>", "\n")
        .replace("<br>", "\n")
        .replace("</p>", "\n");
    for c in replaced.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let mut out = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    // strip [bbcode] tags
    let mut clean = String::with_capacity(out.len());
    let mut chars = out.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '[' {
            let rest: String = chars.clone().take(20).collect();
            if let Some(end) = rest.find(']') {
                let inner = &rest[..end];
                if inner.trim_start_matches('/').chars().all(|x| x.is_ascii_alphanumeric() || x == '=' || x == '#' || x == '"' || x == ' ') && !inner.is_empty() {
                    for _ in 0..=end {
                        chars.next();
                    }
                    continue;
                }
            }
        }
        clean.push(c);
    }
    out = clean;
    out.trim().to_string()
}

/// Strip ModWorkshop's markdown-ish markup to plain text.
pub fn mws_markup_to_text(s: &str) -> String {
    let mut t = s.replace("**", "").replace("__", "").replace("!!!", "");
    // {#hex}(text) colour syntax → text
    while let Some(i) = t.find("{#") {
        if let Some(j) = t[i..].find("}(") {
            let start = i + j + 2;
            if let Some(k) = t[start..].find(')') {
                let inner = t[start..start + k].to_string();
                t.replace_range(i..start + k + 1, &inner);
                continue;
            }
        }
        break;
    }
    t.lines()
        .map(|l| l.trim_start_matches('#').trim_start())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod live_tests {
    //! Hit the real APIs. Run with `cargo test -- --ignored`.
    use super::*;

    #[test]
    #[ignore]
    fn mws_search_and_files() {
        let (mods, more) = mws::search(&mws::Query { text: "hud".into(), sort: "downloads", tags: vec![], category: None, page: 1 }).unwrap();
        assert!(!mods.is_empty() && more);
        println!("MWS top: {} by {} thumb={:?} tags={:?}", mods[0].name, mods[0].author, mods[0].thumb, mods[0].tags.iter().map(|t| &t.name).collect::<Vec<_>>());
        let (tagged, _) = mws::search(&mws::Query { text: String::new(), sort: "bumped_at", tags: vec![34], category: None, page: 1 }).unwrap();
        assert!(tagged.iter().all(|m| m.tags.iter().any(|t| t.id == Some(34))));
        assert!(!mws::tags().unwrap().is_empty());
        assert!(mws::categories().unwrap().len() > 10);
        let files = mws::files(mods[0].id).unwrap();
        println!("files: {:?}", files);
    }

    #[test]
    #[ignore]
    fn nexus_search_hides_adult() {
        let q = nexus::Query { text: "hud".into(), sort: "endorsements", category: None, hide_adult: true, offset: 0 };
        let (mods, _) = nexus::search(&q).unwrap();
        assert!(!mods.is_empty());
        assert!(mods.iter().all(|m| !m.nsfw), "adult mod leaked through filter");
        let q = nexus::Query { hide_adult: false, ..q };
        let (all, _) = nexus::search(&q).unwrap();
        println!("nexus: {} safe / {} total, nsfw in unfiltered: {}", mods.len(), all.len(), all.iter().filter(|m| m.nsfw).count());
        let d = nexus::detail(mods[0].id).unwrap();
        assert_eq!(d.id, mods[0].id);
        assert!(!nexus::files(mods[0].id).unwrap().is_empty());
    }

    #[test]
    #[ignore]
    fn install_real_mod_into_fake_game() {
        let game = std::env::temp_dir().join(format!("pd2mm_fakegame_{}", crate::installer::now_nanos()));
        std::fs::create_dir_all(game.join("mods/base")).unwrap();
        std::fs::write(game.join("PAYDAY2.exe"), "").unwrap();
        std::fs::write(game.join("WSOCK32.dll"), "").unwrap();
        // Find a popular ModWorkshop mod with a direct file and install it.
        let (mods, _) = mws::search(&mws::Query { text: String::new(), sort: "downloads", tags: vec![34], category: None, page: 1 }).unwrap();
        let mut done = false;
        for m in mods.iter().take(10) {
            let files = mws::files(m.id).unwrap();
            let Some(f) = files.iter().find(|f| matches!(f.kind, crate::model::FileKind::Direct(_)) && f.size < 20_000_000) else { continue };
            let crate::model::FileKind::Direct(url) = &f.kind else { continue };
            let arc = crate::config::temp_dir().join("live_test_dl");
            crate::installer::download(url, &arc, &|_, _| {}).unwrap();
            let r = crate::installer::install_archive(&game, &arc, Some(m), crate::model::Source::ModWorkshop, &f.version, &m.name, &|_, _| {}).unwrap();
            for im in &r.mods {
                println!("installed '{}' as {:?} -> {}", m.name, im.kind, crate::game::mod_path(&game, im).display());
                assert!(crate::game::mod_path(&game, im).exists());
            }
            let mut db = crate::config::ModDb::default();
            for im in r.mods {
                db.mods.insert(im.key.clone(), im);
            }
            crate::game::scan(&game, &mut db);
            let key = db.mods.keys().next().unwrap().clone();
            let mut im = db.mods[&key].clone();
            if im.kind != crate::model::ModKind::Root {
                crate::game::set_enabled(&game, &mut im, false).unwrap();
                assert!(crate::game::mod_path(&game, &im).exists());
                crate::game::set_enabled(&game, &mut im, true).unwrap();
                crate::game::uninstall(&game, &im).unwrap();
                assert!(!crate::game::mod_path(&game, &im).exists());
            }
            done = true;
            break;
        }
        let _ = std::fs::remove_dir_all(&game);
        assert!(done, "no installable mod found");
    }
}

#[cfg(test)]
mod dep_tests {
    use super::*;

    #[test]
    fn dep_links() {
        let d = dep_from_url("LAS", "https://modworkshop.net/mod/24006", false);
        assert_eq!((d.source, d.id), (Source::ModWorkshop, Some(24006)));
        let d = dep_from_url("X", "https://www.nexusmods.com/payday2/mods/359?tab=files", true);
        assert_eq!((d.source, d.id, d.optional), (Source::Nexus, Some(359), true));
        let d = dep_from_url("SuperBLT", "https://superblt.znix.xyz/", false);
        assert_eq!((d.source, d.id), (Source::Local, None));
    }

    #[test]
    #[ignore]
    fn live_dependencies() {
        // Animated skin pack: needs Animated Weapon Skins (mod dep) + SuperBLT (template dep)
        let p = mws::primary(59979).unwrap();
        println!("59979: {:?}", p.deps.iter().map(|d| (&d.name, d.id, d.optional)).collect::<Vec<_>>());
        assert!(p.deps.iter().any(|d| d.id == Some(59388) && !d.optional));
        // Legendary Armour Skins: template says BeardLib
        let p = mws::primary(24006).unwrap();
        println!("24006: {:?}", p.deps.iter().map(|d| (&d.name, d.id, d.optional)).collect::<Vec<_>>());
        assert!(p.deps.iter().any(|d| d.id == Some(14924)));
        // Extra Heist Info: optional dependency must stay optional
        let p = mws::primary(31915).unwrap();
        println!("31915: {:?}", p.deps.iter().map(|d| (&d.name, d.id, d.optional)).collect::<Vec<_>>());
        // Nexus mod 225 requires Legendary Armour Skins via a ModWorkshop link
        let r = nexus::requirements(225).unwrap();
        println!("nexus 225: {:?}", r);
        assert!(r.iter().any(|d| d.source == Source::ModWorkshop && d.id == Some(24006)));
    }
}
