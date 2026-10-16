//! A fake release served from a local HTTP server (std `TcpListener`):
//! check → download → verify → unpack → swap into a temp "install". No
//! network beyond 127.0.0.1.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rugen_update::{Install, Phase, Updater, apply};

/// Serve `files` (path → body) on `listener` until `hits` requests were answered.
fn serve_on(listener: TcpListener, files: HashMap<String, Vec<u8>>, hits: usize) {
    std::thread::spawn(move || {
        for _ in 0..hits {
            let Ok((mut s, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or("/")
                .to_string();
            let (status, body) = match files.get(&path) {
                Some(b) => ("200 OK", b.clone()),
                None => ("404 Not Found", b"nope".to_vec()),
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(&body);
        }
    });
}

/// Serve `files` on a fresh port; the base URL.
fn serve(files: HashMap<String, Vec<u8>>, hits: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    serve_on(listener, files, hits);
    format!("http://127.0.0.1:{port}")
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("rugen-update-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The release tree as the workflow packs it, under `top/` when `top` is set.
fn release_files(top: &str) -> Vec<(String, Vec<u8>, bool)> {
    let p = |s: &str| {
        if top.is_empty() {
            s.to_string()
        } else {
            format!("{top}/{s}")
        }
    };
    vec![
        (
            p(rugen_update::exe_name()),
            b"#!/bin/sh\necho new\n".to_vec(),
            true,
        ),
        (p("README.md"), b"new readme".to_vec(), false),
        (p("motif/new.txt"), b"new motif".to_vec(), false),
        (p("motif/sub/deep.txt"), b"deep".to_vec(), false),
        (p("data/common1.cns"), b"new data".to_vec(), false),
        (p("chars/README.md"), b"new chars readme".to_vec(), false),
        (p("stages/README.md"), b"new stages readme".to_vec(), false),
    ]
}

fn tar_gz(files: &[(String, Vec<u8>, bool)]) -> Vec<u8> {
    let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut tar = tar::Builder::new(enc);
    for (path, body, exec) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(body.len() as u64);
        h.set_mode(if *exec { 0o755 } else { 0o644 });
        h.set_cksum();
        tar.append_data(&mut h, path, body.as_slice()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

fn zip_bytes(files: &[(String, Vec<u8>, bool)]) -> Vec<u8> {
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (path, body, exec) in files {
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(if *exec { 0o755 } else { 0o644 });
        out.start_file(path.as_str(), opts).unwrap();
        out.write_all(body).unwrap();
    }
    out.finish().unwrap().into_inner()
}

fn fake_install(dir: &Path) {
    std::fs::write(dir.join(rugen_update::exe_name()), b"old binary").unwrap();
    std::fs::create_dir_all(dir.join("motif/sub")).unwrap();
    std::fs::write(dir.join("motif/old.txt"), b"old motif").unwrap();
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(dir.join("data/common1.cns"), b"old data").unwrap();
    std::fs::create_dir_all(dir.join("chars")).unwrap();
    std::fs::write(dir.join("chars/mine.rgc"), b"my fighter").unwrap();
    std::fs::write(dir.join("chars/README.md"), b"old chars readme").unwrap();
    std::fs::create_dir_all(dir.join("stages")).unwrap();
    std::fs::write(dir.join("rugen.toml"), b"[video]\nfullscreen = true\n").unwrap();
    std::fs::create_dir_all(dir.join("cache")).unwrap();
    std::fs::write(dir.join("cache/x.bin"), b"cache").unwrap();
    std::fs::write(dir.join("README.md"), b"old readme").unwrap();
}

/// The fake release: JSON, archive for this machine, SHA256SUMS (`tamper`
/// writes a wrong checksum).
fn release(base_hint: &str, tamper: bool) -> (HashMap<String, Vec<u8>>, String) {
    let name = rugen_update::own_asset_name("0.8.0").expect("a build for this test machine");
    let archive = if name.ends_with(".zip") {
        zip_bytes(&release_files(""))
    } else {
        tar_gz(&release_files("rugen-0.8.0-linux-x86_64"))
    };
    let mut sha = rugen_update::sha256_hex(&archive);
    if tamper {
        sha = sha.chars().rev().collect();
    }
    let sums = format!(
        "{sha}  {name}\n0000000000000000000000000000000000000000000000000000000000000000  other.zip\n"
    );
    let json = serde_json::json!({
        "tag_name": "v0.8.0",
        "published_at": "2026-10-20T08:14:42Z",
        "html_url": format!("{base_hint}/tag/v0.8.0"),
        "assets": [
            {"name": name, "browser_download_url": format!("{base_hint}/{name}"), "size": archive.len()},
            {"name": "SHA256SUMS", "browser_download_url": format!("{base_hint}/SHA256SUMS"), "size": sums.len()},
            {"name": "rugen_0.8.0-1_amd64.deb", "browser_download_url": format!("{base_hint}/rugen_0.8.0-1_amd64.deb"), "size": 3}
        ]
    });
    let mut files = HashMap::new();
    files.insert("/latest".to_string(), serde_json::to_vec(&json).unwrap());
    files.insert(format!("/{name}"), archive);
    files.insert("/SHA256SUMS".to_string(), sums.into_bytes());
    (files, name)
}

/// Two passes: the server's port is only known after binding, so the JSON
/// points at a placeholder host that is rewritten once the port is known.
fn serve_release(tamper: bool, hits: usize) -> (String, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let base = format!("http://127.0.0.1:{port}");
    let (files, name) = release(&base, tamper);
    // bind the same port again right away
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).unwrap();
    serve_on(listener, files, hits);
    (base, name)
}

#[test]
fn check_download_verify_and_swap_a_portable_install() {
    let root = temp("swap");
    let install = root.join("install");
    std::fs::create_dir_all(&install).unwrap();
    fake_install(&install);
    let (base, _) = serve_release(false, 8);
    let up = Updater::new(
        "0.7.2",
        Install::Portable(install.clone()),
        root.join("cache/update.json"),
        format!("{base}/latest"),
    );
    assert_eq!(up.snapshot().phase, Phase::Idle);
    assert!(up.due());
    up.check_now();
    let snap = up.snapshot();
    assert_eq!(snap.phase, Phase::Available, "{:?}", snap.error);
    let latest = snap.newer().expect("newer release");
    assert_eq!(latest.version, "0.8.0");
    assert_eq!(latest.date, "2026-10-20");
    assert!(root.join("cache/update.json").is_file(), "check cached");
    assert_eq!(up.blocker(), None);

    // a second updater reads the cache: no network needed to know
    let again = Updater::new(
        "0.7.2",
        Install::Portable(install.clone()),
        root.join("cache/update.json"),
        "http://127.0.0.1:9/never".into(),
    );
    assert_eq!(again.snapshot().phase, Phase::Available);
    assert!(!again.due());
    assert!(
        !Arc::clone(&again).check(false),
        "fresh cache: no check started"
    );
    let current = Updater::new(
        "0.8.0",
        Install::Portable(install.clone()),
        root.join("cache/update.json"),
        "http://127.0.0.1:9/never".into(),
    );
    assert_eq!(current.snapshot().phase, Phase::UpToDate);

    up.apply_now();
    let snap = up.snapshot();
    assert_eq!(snap.phase, Phase::Ready, "{:?}", snap.error);
    assert!(snap.progress >= 1.0);
    let read = |p: &str| std::fs::read_to_string(install.join(p)).unwrap();
    assert_eq!(read(rugen_update::exe_name()), "#!/bin/sh\necho new\n");
    assert_eq!(read("motif/new.txt"), "new motif");
    assert_eq!(read("motif/sub/deep.txt"), "deep");
    assert!(
        !install.join("motif/old.txt").exists(),
        "motif/ replaced whole"
    );
    assert_eq!(read("data/common1.cns"), "new data");
    assert_eq!(read("README.md"), "new readme");
    assert_eq!(read("chars/README.md"), "new chars readme");
    assert_eq!(read("stages/README.md"), "new stages readme");
    // player files untouched
    assert_eq!(read("chars/mine.rgc"), "my fighter");
    assert_eq!(read("rugen.toml"), "[video]\nfullscreen = true\n");
    assert_eq!(read("cache/x.bin"), "cache");
    assert!(!install.join(apply::STAGE_DIR).exists());
    assert!(!install.join(apply::OLD_DIR).exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(install.join("rugen"))
            .unwrap()
            .permissions()
            .mode();
        assert!(mode & 0o111 != 0, "binary stays executable ({mode:o})");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_checksum_mismatch_leaves_the_install_alone() {
    let root = temp("tamper");
    let install = root.join("install");
    std::fs::create_dir_all(&install).unwrap();
    fake_install(&install);
    let (base, _) = serve_release(true, 4);
    let up = Updater::new(
        "0.7.2",
        Install::Portable(install.clone()),
        root.join("cache/update.json"),
        format!("{base}/latest"),
    );
    up.check_now();
    assert_eq!(up.snapshot().phase, Phase::Available);
    up.apply_now();
    let snap = up.snapshot();
    assert_eq!(snap.phase, Phase::Failed);
    assert!(
        snap.error.as_deref().unwrap_or("").contains("SHA256SUMS"),
        "{:?}",
        snap.error
    );
    let read = |p: &str| std::fs::read_to_string(install.join(p)).unwrap();
    assert_eq!(read(rugen_update::exe_name()), "old binary");
    assert_eq!(read("motif/old.txt"), "old motif");
    assert!(!install.join(apply::STAGE_DIR).exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_package_install_never_writes() {
    let root = temp("pkg");
    let (base, _) = serve_release(false, 2);
    let up = Updater::new(
        "0.7.2",
        Install::Package(PathBuf::from("/usr/bin/rugen")),
        root.join("cache/update.json"),
        format!("{base}/latest"),
    );
    up.check_now();
    assert_eq!(up.snapshot().phase, Phase::Available);
    assert_eq!(
        up.blocker().as_deref(),
        Some("installed by your package manager")
    );
    assert!(!Arc::clone(&up).start_apply());
    let deb = up
        .snapshot()
        .latest
        .unwrap()
        .package(rugen_update::PackageKind::Deb)
        .unwrap()
        .name
        .clone();
    assert_eq!(deb, "rugen_0.8.0-1_amd64.deb");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_failed_check_keeps_the_error() {
    let root = temp("down");
    let base = serve(HashMap::new(), 1);
    let up = Updater::new(
        "0.7.2",
        Install::Dev(root.clone()),
        root.join("cache/update.json"),
        format!("{base}/latest"),
    );
    up.check_now();
    let snap = up.snapshot();
    assert_eq!(snap.phase, Phase::Failed);
    assert!(snap.error.as_deref().unwrap_or("").contains("404"));
    assert!(snap.latest.is_none());
    let _ = std::fs::remove_dir_all(&root);
}
