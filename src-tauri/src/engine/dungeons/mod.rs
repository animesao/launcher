//! Minecraft Dungeons (the first game) next to Java Edition.
//!
//! The game files come from Mojang's own CDN, the same manifest the official
//! Minecraft Launcher reads; nothing is mirrored on our side. The game itself
//! checks the licence: Themida DRM and the Microsoft account on start. Mods are
//! Unreal .pak files in `Paks/~mods`, which the engine mounts after the base paks.

use crate::engine::*;
use futures::StreamExt;
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::AppHandle;

const INDEX: &str =
    "https://piston-meta.mojang.com/v1/products/dungeons/f4c685912beb55eb2d5c9e0713fe1195164bba27/windows-x64.json";
const ENTITLEMENTS: &str = "https://api.minecraftservices.com/entitlements/mcstore";
const JOB_KEY: &str = "dungeons";
const PARALLEL: usize = 8;
const EXE: &str = "Dungeons.exe";
const MODS_DIR: &str = "Dungeons/Content/Paks/~mods";
const VERSION_FILE: &str = ".millida-version";
const LEDGER_FILE: &str = ".millida-files.json";
const DISABLED: &str = ".disabled";

pub fn dungeons_dir() -> PathBuf {
    game_root().join("dungeons")
}

fn mods_dir() -> PathBuf {
    dungeons_dir().join(MODS_DIR)
}

#[derive(Serialize)]
pub struct DungeonsMod {
    pub name: String,
    pub enabled: bool,
    pub size: u64,
}

#[derive(Serialize)]
pub struct DungeonsStatus {
    /// The game ships for Windows only; elsewhere the screen shows why.
    pub supported: bool,
    pub installed: bool,
    pub version: String,
    pub dir: String,
    pub mods: Vec<DungeonsMod>,
}

pub fn dungeons_status() -> DungeonsStatus {
    let dir = dungeons_dir();
    let version = std::fs::read_to_string(dir.join(VERSION_FILE)).unwrap_or_default().trim().to_string();
    DungeonsStatus {
        supported: cfg!(target_os = "windows"),
        installed: !version.is_empty() && dir.join(EXE).is_file(),
        version,
        dir: dir.to_string_lossy().into_owned(),
        mods: list_mods(),
    }
}

/// Entitlement names carry the product in them (`product_dungeons`,
/// `product_legends`, …). Dungeons II is told apart from the first game by
/// its own marker; Game Pass for PC unlocks all three spin-offs.
fn owns_game(entitlements: &Value, slug: &str) -> bool {
    entitlements["items"].as_array().is_some_and(|items| {
        items.iter().filter_map(|i| i["name"].as_str()).any(|n| {
            let n = n.to_ascii_lowercase().replace(['-', ' '], "_");
            let sequel = ["dungeons2", "dungeons_2", "dungeonsii", "dungeons_ii"].iter().any(|m| n.contains(m));
            n.contains("game_pass_pc")
                || match slug {
                    "dungeons" => n.contains("dungeons") && !sequel,
                    "dungeons-2" => sequel,
                    "legends" => n.contains("legends"),
                    // С 07.06.2022 Java и Bedrock продаются вместе: у владельца Java есть и Bedrock.
                    "bedrock" => n.contains("bedrock") || n == "product_minecraft" || n == "game_minecraft",
                    _ => false,
                }
        })
    })
}

/// `owned`, `not_owned`, `none` (no Microsoft session stored) or `unavailable`.
pub async fn game_ownership(account_id: &str, slug: &str) -> Result<Value, String> {
    let Some(token) = mc_token(account_id) else {
        return Ok(serde_json::json!({ "status": "none" }));
    };
    let r = client().get(ENTITLEMENTS).bearer_auth(&token).send().await.map_err(|e| net_err(&e))?;
    if r.status().as_u16() == 401 {
        return Ok(serde_json::json!({ "status": "none" }));
    }
    if !r.status().is_success() {
        return Ok(serde_json::json!({ "status": "unavailable" }));
    }
    let j: Value = r.json().await.map_err(|e| e.to_string())?;
    let status = if owns_game(&j, slug) { "owned" } else { "not_owned" };
    // Product names only (product_dungeons, …): no ids, signatures or tokens.
    // They go to telemetry so the matcher can be checked against real
    // purchases of Dungeons II and Legends without a Windows test machine.
    let items: Vec<&str> = j["items"]
        .as_array()
        .map(|a| a.iter().filter_map(|i| i["name"].as_str()).filter(|n| n.len() <= 64).collect())
        .unwrap_or_default();
    Ok(serde_json::json!({ "status": status, "items": items }))
}

pub async fn dungeons_ownership(account_id: &str) -> Result<Value, String> {
    game_ownership(account_id, "dungeons").await
}

struct Blob {
    url: String,
    sha1: String,
    size: u64,
}

struct FileJob {
    rel: String,
    path: PathBuf,
    raw: Blob,
    /// Mojang keeps an LZMA copy of most files: 2,33 GB instead of 5,82 GB for
    /// the whole game (manifest 12688467, 29.09.2026). Unpacked on our side.
    lzma: Option<Blob>,
}

impl FileJob {
    /// Bytes that actually cross the network for this file.
    fn wire(&self) -> u64 {
        self.lzma.as_ref().map(|b| b.size).unwrap_or(self.raw.size)
    }
}

fn blob(v: &Value) -> Option<Blob> {
    Some(Blob {
        url: v["url"].as_str()?.to_string(),
        sha1: v["sha1"].as_str().unwrap_or_default().to_string(),
        size: v["size"].as_u64().unwrap_or(0),
    })
}

/// Directories are created up front; files are returned for download. Paths
/// come from Mojang, but they still go through safe_join like any remote path.
fn plan(manifest: &Value, root: &Path) -> Result<Vec<FileJob>, String> {
    let files = manifest["files"].as_object().ok_or("в манифесте Dungeons нет списка файлов")?;
    let mut jobs = Vec::new();
    for (rel, f) in files {
        // Mojang's manifest lists the install root itself as a directory with
        // an empty path; the root already exists.
        if f["type"].as_str() == Some("directory") && rel.trim().is_empty() {
            continue;
        }
        let path = safe_join(root, rel)?;
        if f["type"].as_str() == Some("directory") {
            std::fs::create_dir_all(&path).map_err(|e| format!("{}: {}", path.display(), e))?;
            continue;
        }
        let raw = blob(&f["downloads"]["raw"]).ok_or_else(|| format!("{}: нет ссылки", rel))?;
        let lzma = blob(&f["downloads"]["lzma"]).filter(|b| !b.sha1.is_empty() && b.size < raw.size);
        jobs.push(FileJob { rel: rel.clone(), path, raw, lzma });
    }
    // Largest first: the two 1,2 GB paks set the finish time, so they must not
    // start last behind two hundred small files.
    jobs.sort_by_key(|j| std::cmp::Reverse(j.wire()));
    Ok(jobs)
}

/// What the last install put in place: path → sha1. An update then skips a
/// matching file by its size alone instead of rehashing six gigabytes.
fn read_ledger(root: &Path) -> std::collections::HashMap<String, String> {
    std::fs::read_to_string(root.join(LEDGER_FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn sha1_file(path: &Path) -> Option<String> {
    use sha1::Digest as _;
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = sha1::Sha1::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = std::io::Read::read(&mut f, &mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(hex(&h.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Already in place: same size and either recorded by the ledger or hashing
/// to the manifest sha1 (an install made before the ledger existed).
fn in_place(j: &FileJob, ledger: &std::collections::HashMap<String, String>) -> bool {
    let Ok(meta) = std::fs::metadata(&j.path) else { return false };
    if meta.len() != j.raw.size {
        return false;
    }
    if ledger.get(&j.rel).is_some_and(|s| s.eq_ignore_ascii_case(&j.raw.sha1)) {
        return true;
    }
    sha1_file(&j.path).is_some_and(|s| s.eq_ignore_ascii_case(&j.raw.sha1))
}

struct HashingWriter<W: std::io::Write> {
    inner: W,
    h: sha1::Sha1,
    n: u64,
}

impl<W: std::io::Write> std::io::Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        use sha1::Digest as _;
        let n = self.inner.write(buf)?;
        self.h.update(&buf[..n]);
        self.n += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Unpacks a downloaded .lzma next to its destination, checks the result
/// against the manifest's raw sha1 and size, and only then puts it in place.
fn unpack_lzma(src: &Path, dest: &Path, raw: &Blob) -> Result<(), String> {
    use sha1::Digest as _;
    let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dest.with_file_name(format!("{}.millida-unpack", name));
    let res = (|| {
        let input = std::fs::File::open(src).map_err(|e| format!("{}: {}", src.display(), e))?;
        let mut input = std::io::BufReader::with_capacity(1 << 20, input);
        let out = std::fs::File::create(&tmp).map_err(|e| format!("{}: {}", tmp.display(), e))?;
        let mut w = HashingWriter { inner: std::io::BufWriter::with_capacity(1 << 20, out), h: sha1::Sha1::new(), n: 0 };
        lzma_rs::lzma_decompress(&mut input, &mut w).map_err(|e| format!("{}: не распаковался ({})", name, e))?;
        std::io::Write::flush(&mut w).map_err(|e| e.to_string())?;
        let HashingWriter { inner, h, n } = w;
        drop(inner);
        let got = hex(&h.finalize());
        if n != raw.size || !got.eq_ignore_ascii_case(&raw.sha1) {
            return Err(format!("{}: контрольная сумма не сошлась после распаковки", name));
        }
        std::fs::rename(&tmp, dest).map_err(|e| format!("{}: {}", dest.display(), e))
    })();
    let _ = std::fs::remove_file(src);
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

async fn fetch_one(j: &FileJob, cancel: &std::sync::atomic::AtomicBool) -> Result<(), String> {
    let Some(z) = &j.lzma else {
        let sum = (!j.raw.sha1.is_empty()).then_some(Sum::Sha1(j.raw.sha1.as_str()));
        return download_checked_cancellable(&j.raw.url, &j.path, sum, Some(j.raw.size), Some(cancel)).await;
    };
    let name = j.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let packed = j.path.with_file_name(format!("{}.millida-lzma", name));
    download_checked_cancellable(&z.url, &packed, Some(Sum::Sha1(z.sha1.as_str())), Some(z.size), Some(cancel)).await?;
    let (dest, raw) = (j.path.clone(), Blob { url: String::new(), sha1: j.raw.sha1.clone(), size: j.raw.size });
    tokio::task::spawn_blocking(move || unpack_lzma(&packed, &dest, &raw))
        .await
        .map_err(|e| format!("распаковка прервалась: {}", e))?
}

/// Installs or updates the game in place. Files already in place are kept, so
/// an update downloads only what changed, and `~mods` is never touched: it is
/// not in Mojang's manifest.
pub async fn dungeons_install(app: AppHandle, account_id: String) -> Result<String, String> {
    let job = Job::start(JOB_KEY, "Minecraft Dungeons")?;
    let res = install_inner(&app, &job, &account_id).await;
    job.finish(&app, res)
}

async fn install_inner(app: &AppHandle, job: &Job, account_id: &str) -> Result<String, String> {
    // Покупку не проверяем (владелец 29.09.2026): игра под защитой Themida и
    // сама спрашивает аккаунт Microsoft при запуске (pcgamingwiki.com).
    let _ = account_id;
    job.emit(app, 1.0, "Получаем список файлов");
    let index = get_json(INDEX).await?;
    let latest = &index["dungeons"][0];
    let version = latest["version"]["name"].as_str().unwrap_or("unknown").to_string();
    let manifest_url = latest["manifest"]["url"].as_str().ok_or("Mojang не отдал манифест Dungeons")?;
    let manifest = get_json(manifest_url).await?;

    let root = dungeons_dir();
    std::fs::create_dir_all(&root).map_err(|e| format!("{}: {}", root.display(), e))?;
    let all = plan(&manifest, &root)?;

    job.emit(app, 2.0, "Проверяем файлы");
    let ledger = read_ledger(&root);
    let (all, todo): (Vec<FileJob>, Vec<bool>) = tokio::task::spawn_blocking(move || {
        let todo = all.iter().map(|j| !in_place(j, &ledger)).collect::<Vec<_>>();
        (all, todo)
    })
    .await
    .map_err(|e| e.to_string())?;
    let mut record: std::collections::HashMap<String, String> =
        all.iter().map(|j| (j.rel.clone(), j.raw.sha1.clone())).collect();
    let jobs: Vec<FileJob> = all.into_iter().zip(todo).filter(|(_, t)| *t).map(|(j, _)| j).collect();

    let total: u64 = jobs.iter().map(|j| j.wire()).sum::<u64>().max(1);
    let done = Arc::new(AtomicU64::new(0));
    let gb = |b: u64| b as f64 / 1_073_741_824.0;
    let results: Vec<(String, Result<(), String>)> = futures::stream::iter(jobs.into_iter().map(|j| {
        let done = done.clone();
        async move {
            let r = fetch_one(&j, job.cancel_flag()).await;
            let d = done.fetch_add(j.wire(), Ordering::Relaxed) + j.wire();
            job.emit(app, 3.0 + 96.0 * (d as f32 / total as f32), &format!("{:.2} / {:.2} ГБ", gb(d), gb(total)));
            (j.rel.clone(), r)
        }
    }))
    .buffer_unordered(PARALLEL)
    .collect()
    .await;
    job.check()?;
    let mut first_err = None;
    for (rel, r) in results {
        if let Err(e) = r {
            record.remove(&rel);
            first_err.get_or_insert(e);
        }
    }
    write_json_quiet(&root.join(LEDGER_FILE), &record);
    if let Some(e) = first_err {
        return Err(e);
    }

    std::fs::create_dir_all(mods_dir()).map_err(|e| e.to_string())?;
    std::fs::write(root.join(VERSION_FILE), &version).map_err(|e| e.to_string())?;
    job.emit(app, 100.0, "Готово");
    Ok(version)
}

pub fn dungeons_launch() -> Result<(), String> {
    if !cfg!(target_os = "windows") {
        return Err("Minecraft Dungeons запускается только на Windows".into());
    }
    let dir = dungeons_dir();
    let exe = dir.join(EXE);
    if !exe.is_file() {
        return Err("Игра не установлена".into());
    }
    let mut cmd = std::process::Command::new(&exe);
    cmd.current_dir(&dir);
    quiet(&mut cmd).spawn().map_err(|e| format!("Не удалось запустить игру: {}", e))?;
    Ok(())
}

fn list_mods() -> Vec<DungeonsMod> {
    let Ok(rd) = std::fs::read_dir(mods_dir()) else { return Vec::new() };
    let mut out: Vec<DungeonsMod> = rd
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let lower = name.to_ascii_lowercase();
            let enabled = lower.ends_with(".pak");
            if !enabled && !lower.ends_with(&format!(".pak{}", DISABLED)) {
                return None;
            }
            let base = name.strip_suffix(DISABLED).unwrap_or(&name).to_string();
            Some(DungeonsMod { name: base, enabled, size: e.metadata().map(|m| m.len()).unwrap_or(0) })
        })
        .collect();
    out.sort_by_key(|m| m.name.to_ascii_lowercase());
    out
}

fn pak_name(name: &str) -> Result<String, String> {
    let name = safe_file_name(name)?;
    if !name.to_ascii_lowercase().ends_with(".pak") {
        return Err("Мод Dungeons — это файл .pak".into());
    }
    Ok(name)
}

pub fn dungeons_add_mods(paths: Vec<PathBuf>) -> Result<usize, String> {
    let dir = mods_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    let mut added = 0;
    for src in paths {
        let name = pak_name(&src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())?;
        let dest = safe_child(&dir, &name)?;
        let _ = std::fs::remove_file(safe_child(&dir, &format!("{}{}", name, DISABLED))?);
        std::fs::copy(&src, &dest).map_err(|e| format!("{}: {}", src.display(), e))?;
        added += 1;
    }
    Ok(added)
}

pub fn dungeons_toggle_mod(name: &str, enabled: bool) -> Result<(), String> {
    let name = pak_name(name)?;
    let dir = mods_dir();
    let on = safe_child(&dir, &name)?;
    let off = safe_child(&dir, &format!("{}{}", name, DISABLED))?;
    let (from, to) = if enabled { (off, on) } else { (on, off) };
    if !from.exists() {
        return Ok(());
    }
    std::fs::rename(&from, &to).map_err(|e| e.to_string())
}

pub fn dungeons_remove_mod(name: &str) -> Result<(), String> {
    let name = pak_name(name)?;
    let dir = mods_dir();
    for p in [safe_child(&dir, &name)?, safe_child(&dir, &format!("{}{}", name, DISABLED))?] {
        if p.exists() {
            std::fs::remove_file(&p).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_is_per_game() {
        let d1 = serde_json::json!({"items":[{"name":"product_minecraft"},{"name":"product_dungeons"}]});
        let d2 = serde_json::json!({"items":[{"name":"product_dungeons2"}]});
        let legends = serde_json::json!({"items":[{"name":"product_legends"}]});
        let pass = serde_json::json!({"items":[{"name":"product_game_pass_pc"}]});
        let java = serde_json::json!({"items":[{"name":"product_minecraft"},{"name":"game_minecraft"}]});
        assert!(owns_game(&d1, "dungeons"));
        assert!(!owns_game(&d1, "dungeons-2"), "the first game does not unlock the sequel");
        assert!(owns_game(&d2, "dungeons-2"));
        assert!(!owns_game(&d2, "dungeons"), "the sequel does not unlock the first game");
        assert!(owns_game(&legends, "legends"));
        assert!(!owns_game(&legends, "dungeons"));
        for g in ["dungeons", "dungeons-2", "legends"] {
            assert!(owns_game(&pass, g));
            assert!(!owns_game(&java, g));
        }
        assert!(!owns_game(&serde_json::json!({}), "dungeons"));
        assert!(owns_game(&java, "bedrock"), "a Java owner owns Bedrock since the 2022 bundle");
        assert!(owns_game(&serde_json::json!({"items":[{"name":"product_minecraft_bedrock"}]}), "bedrock"));
        assert!(!owns_game(&legends, "bedrock"));
    }

    #[test]
    fn plan_rejects_escaping_paths() {
        let root = std::env::temp_dir().join("millida-dungeons-plan-test");
        let m = serde_json::json!({"files":{"../evil.exe":{"type":"file","downloads":{"raw":{"url":"x","sha1":"","size":1}}}}});
        assert!(plan(&m, &root).is_err());
    }

    #[test]
    fn plan_splits_dirs_and_files() {
        let root = std::env::temp_dir().join("millida-dungeons-plan-ok");
        let m = serde_json::json!({"files":{
            "":{"type":"directory"},
            "Dungeons/Content/Paks/":{"type":"directory"},
            "Dungeons.exe":{"type":"file","downloads":{"raw":{"url":"https://a/b","sha1":"ab","size":5}}}
        }});
        let jobs = plan(&m, &root).unwrap();
        assert_eq!(jobs.len(), 1);
        assert!(root.join("Dungeons/Content/Paks").is_dir());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn lzma_copy_is_unpacked_and_verified() {
        use sha1::Digest as _;
        let dir = std::env::temp_dir().join("millida-dungeons-lzma-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let mut packed = Vec::new();
        lzma_rs::lzma_compress(&mut std::io::Cursor::new(&data), &mut packed).unwrap();
        let src = dir.join("a.bin.millida-lzma");
        std::fs::write(&src, &packed).unwrap();
        let dest = dir.join("a.bin");
        let good = Blob { url: String::new(), sha1: hex(&sha1::Sha1::digest(&data)), size: data.len() as u64 };
        unpack_lzma(&src, &dest, &good).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert!(!src.exists(), "the packed copy is removed after unpacking");

        std::fs::write(&src, &packed).unwrap();
        let bad = Blob { url: String::new(), sha1: "00".repeat(20), size: data.len() as u64 };
        let dest2 = dir.join("b.bin");
        assert!(unpack_lzma(&src, &dest2, &bad).is_err());
        assert!(!dest2.exists(), "a file that fails the checksum never lands in place");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Real Mojang file: MILLIDA_LZMA_SAMPLE=path/to/Dungeons.exe.lzma
    /// (piston-data object bb4766f2…, unpacks to sha1 30a9d128…, 189952 bytes).
    #[test]
    #[ignore]
    fn mojang_lzma_sample_unpacks() {
        let Ok(sample) = std::env::var("MILLIDA_LZMA_SAMPLE") else { return };
        let dir = std::env::temp_dir().join("millida-dungeons-lzma-real");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("Dungeons.exe.millida-lzma");
        std::fs::copy(sample, &src).unwrap();
        let raw = Blob { url: String::new(), sha1: "30a9d1283e0a1f3f7590f1926d55c8740b21a111".into(), size: 189952 };
        unpack_lzma(&src, &dir.join("Dungeons.exe"), &raw).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Живая проверка против серверов Mojang: MILLIDA_DUNGEONS_LIVE=1.
    /// Берёт настоящий манифест, качает самые мелкие файлы — сжатые и обычный —
    /// и один средний .lzma, распаковывает и сверяет с манифестом.
    #[test]
    #[ignore]
    fn live_mojang_manifest_downloads() {
        if std::env::var("MILLIDA_DUNGEONS_LIVE").is_err() {
            return;
        }
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let index = get_json(INDEX).await.unwrap();
            let url = index["dungeons"][0]["manifest"]["url"].as_str().unwrap().to_string();
            let manifest = get_json(&url).await.unwrap();
            let root = std::env::temp_dir().join("millida-dungeons-live");
            let _ = std::fs::remove_dir_all(&root);
            let mut jobs = plan(&manifest, &root).unwrap();
            assert!(jobs.len() > 200, "the manifest lists the whole game");
            let wire: u64 = jobs.iter().map(|j| j.wire()).sum();
            let raw: u64 = jobs.iter().map(|j| j.raw.size).sum();
            println!("wire {} MB of raw {} MB", wire / 1_000_000, raw / 1_000_000);
            assert!(wire * 2 < raw, "LZMA copies at least halve the download");
            jobs.sort_by_key(|j| j.wire());
            let mut pick: Vec<FileJob> = Vec::new();
            let mut z = 0;
            let mut plain = 0;
            for j in jobs.into_iter() {
                if j.lzma.is_some() && z < 3 {
                    z += 1;
                    pick.push(j);
                } else if j.lzma.is_none() && plain < 1 {
                    plain += 1;
                    pick.push(j);
                } else if j.lzma.as_ref().is_some_and(|b| b.size > 5_000_000 && b.size < 30_000_000) && pick.len() < 5 {
                    pick.push(j);
                }
            }
            let cancel = std::sync::atomic::AtomicBool::new(false);
            for j in &pick {
                fetch_one(j, &cancel).await.unwrap_or_else(|e| panic!("{}: {}", j.rel, e));
                assert!(in_place(j, &std::collections::HashMap::new()), "{} matches the manifest sha1", j.rel);
                println!("ok {} ({} → {} bytes)", j.rel, j.wire(), j.raw.size);
            }
            let _ = std::fs::remove_dir_all(&root);
        });
    }

    #[test]
    fn plan_prefers_lzma_and_sorts_largest_first() {
        let root = std::env::temp_dir().join("millida-dungeons-plan-lzma");
        let m = serde_json::json!({"files":{
            "small.dll":{"type":"file","downloads":{"raw":{"url":"https://a/s","sha1":"aa","size":10}}},
            "big.pak":{"type":"file","downloads":{
                "raw":{"url":"https://a/r","sha1":"bb","size":1000},
                "lzma":{"url":"https://a/z","sha1":"cc","size":400}}}
        }});
        let jobs = plan(&m, &root).unwrap();
        assert_eq!(jobs[0].rel, "big.pak");
        assert_eq!(jobs[0].wire(), 400);
        assert_eq!(jobs[1].wire(), 10);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_pak_files_are_mods() {
        assert!(pak_name("cool.pak").is_ok());
        assert!(pak_name("cool.PAK").is_ok());
        assert!(pak_name("virus.exe").is_err());
        assert!(pak_name("../x.pak").is_err());
    }
}
