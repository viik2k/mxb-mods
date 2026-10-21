//! mxb-mods: install MX Bikes mods without wrecking their folder layout.
//!
//!   mxb-mods                                     (opens the window)
//!   mxb-mods install <archive | folder | file.pkz>
//!   mxb-mods list
//!
//! Extraction goes through bsdtar (Windows' own tar.exe, or `bsdtar` on SteamOS), which reads
//! zip, rar and 7z. On Linux the mods folder is found inside MX Bikes' Proton prefix.

mod gui;

use std::collections::BTreeSet;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

type R<T> = Result<T, Box<dyn std::error::Error>>;
/// (source file, destination relative to the mods folder)
type Plan = Vec<(PathBuf, PathBuf)>;

const CATEGORIES: [&str; 5] = ["bikes", "tracks", "rider", "tyres", "misc"];

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let mods = mods_dir();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => {
            gui::run(mods);
            return;
        }
        ["install", src] => install(Path::new(src), &mods),
        ["list"] => list(&mods),
        _ => {
            eprintln!("usage: mxb-mods install <archive|folder|file.pkz> | list");
            eprintln!("mods folder: {} (override with MXB_MODS)", mods.display());
            exit(2)
        }
    };
    match result {
        Ok(lines) => lines.iter().for_each(|l| println!("{l}")),
        Err(e) => {
            eprintln!("error: {e}");
            exit(1)
        }
    }
}

const MXB_APPID: &str = "655500";

fn mods_dir() -> PathBuf {
    if let Some(p) = env::var_os("MXB_MODS") {
        return PathBuf::from(p);
    }
    let docs_mods = Path::new("Documents").join("PiBoSo").join("MX Bikes").join("mods");
    if cfg!(windows) {
        return PathBuf::from(env::var_os("USERPROFILE").unwrap_or_default()).join(docs_mods);
    }
    // Steam Deck / Linux: the game runs under Proton, so its Documents folder is inside the
    // prefix, in whichever Steam library (internal or SD card) MX Bikes is installed to.
    let steam = PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".local/share/Steam");
    let vdf = fs::read_to_string(steam.join("steamapps/libraryfolders.vdf")).unwrap_or_default();
    let prefix_mods = |lib: &Path| {
        lib.join("steamapps/compatdata").join(MXB_APPID)
            .join("pfx/drive_c/users/steamuser").join(&docs_mods)
    };
    let mut candidates = library_paths(&vdf);
    candidates.insert(0, steam);
    let all: Vec<_> = candidates.iter().map(|l| prefix_mods(l)).collect();
    // Not found: return the internal-storage path so the error message shows where it looked.
    all.iter().find(|p| p.is_dir()).unwrap_or(&all[0]).clone()
}

/// The `"path"` entries of Steam's libraryfolders.vdf, e.g. the SD card library.
fn library_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|l| {
            let mut quoted = l.split('"').skip(1).step_by(2); // "path"  "/run/media/mmcblk0p1"
            if quoted.next()? != "path" {
                return None;
            }
            Some(PathBuf::from(quoted.next()?))
        })
        .collect()
}

/// Returns one "installed tracks/Foo" line per mod.
fn install(src: &Path, mods: &Path) -> R<Vec<String>> {
    if !mods.is_dir() {
        return Err(format!("mods folder not found: {}", mods.display()).into());
    }
    let tmp = TempDir(env::temp_dir().join(format!("mxb-mods-{}", std::process::id())));
    let plan = if src.is_dir() {
        plan(src, &stem(src))?
    } else if has_ext(src, "pkz") {
        vec![(src.to_path_buf(), Path::new(pkz_kind(src)).join(src.file_name().unwrap()))]
    } else {
        fs::create_dir_all(&tmp.0)?;
        // bsdtar refuses absolute paths and `..` entries by default, so a hostile archive stays in tmp.
        tar(&["-xf".as_ref(), src.as_os_str(), "-C".as_ref(), tmp.0.as_os_str()])?;
        plan(&tmp.0, &stem(src))?
    };

    // Check every destination before copying anything, so a clash leaves the mods folder untouched.
    let clashes: Vec<_> = plan.iter().filter(|(_, d)| mods.join(d).exists()).collect();
    if !clashes.is_empty() {
        let first = clashes[0].1.display();
        return Err(format!("{} file(s) already installed (first: {first}). Remove the old version first.", clashes.len()).into());
    }
    for (from, to) in &plan {
        let to = mods.join(to);
        fs::create_dir_all(to.parent().unwrap())?;
        fs::copy(from, &to)?;
    }
    Ok(summary(&plan).into_iter().map(|item| format!("installed {item}")).collect())
}

/// Work out where each file goes. Order matters: an archive that is already laid out like
/// the mods folder wins, then track folders, then loose .pkz files.
fn plan(root: &Path, name: &str) -> R<Plan> {
    let (dirs, files) = walk(root)?;
    let rel = |f: &PathBuf, base: &Path| f.strip_prefix(base).unwrap().to_path_buf();

    // 1. Archive contains mods/tracks/..., bikes/..., rider/... etc: copy it as laid out.
    //    Archives made on Windows often say "Tracks"; the folder is always written lowercase,
    //    since Linux would otherwise create a second "Tracks" folder the game never reads.
    let in_category = |d: &PathBuf| d.file_name().and_then(category).is_some();
    if let Some(base) = dirs.iter().find(|b| dirs.iter().any(|d| d.parent() == Some(b.as_path()) && in_category(d))) {
        return Ok(files
            .iter()
            .filter_map(|f| {
                let mut parts = f.strip_prefix(base).ok()?.components();
                let cat = category(parts.next()?.as_os_str())?;
                let rest = parts.as_path();
                (rest != Path::new("")).then(|| (f.clone(), Path::new(cat).join(rest)))
            })
            .collect());
    }

    // 2. Tracks: every outermost folder holding a .map. Its layout subfolders (Nat/, SXN/...)
    //    reuse its .map and sky files, so the whole folder must stay together. If the archive
    //    has no top folder (the MXB App / JS7 Compound bug), name it after the archive.
    let mut tracks: Vec<&PathBuf> = vec![];
    for d in &dirs {
        let has_map = files.iter().any(|f| f.parent() == Some(d.as_path()) && has_ext(f, "map"));
        if has_map && !tracks.iter().any(|t| d.starts_with(t)) {
            tracks.push(d);
        }
    }
    if !tracks.is_empty() {
        let mut p = Plan::new();
        for t in tracks {
            let folder = if t == root { OsStr::new(name) } else { t.file_name().unwrap() };
            for f in files.iter().filter(|f| f.starts_with(t)) {
                p.push((f.clone(), Path::new("tracks").join(folder).join(rel(f, t))));
            }
        }
        return Ok(p);
    }

    // 3. Loose .pkz files.
    let p: Plan = files
        .iter()
        .filter(|f| has_ext(f, "pkz"))
        .map(|f| (f.clone(), Path::new(pkz_kind(f)).join(f.file_name().unwrap())))
        .collect();
    if p.is_empty() {
        return Err("don't recognise this mod: no mods-style folders, no track (.map), no .pkz".into());
    }
    Ok(p)
}

fn category(name: &OsStr) -> Option<&'static str> {
    CATEGORIES.into_iter().find(|c| name.eq_ignore_ascii_case(c))
}

/// ponytail: a pkz tar can't read (encrypted OEM bikes) is assumed to be a bike. Peek at the
/// pkz header properly if encrypted track or tyre pkz files turn up.
fn pkz_kind(pkz: &Path) -> &'static str {
    match tar(&["-tf".as_ref(), pkz.as_os_str()]) {
        Ok(listing) if listing.lines().any(|l| l.to_ascii_lowercase().ends_with(".map")) => "tracks",
        _ => "bikes",
    }
}

fn list(mods: &Path) -> R<Vec<String>> {
    let mut out = vec![];
    for c in ["tracks", "bikes", "tyres", "rider"] {
        let dir = mods.join(c);
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        out.push(format!("{c}/"));
        for e in entries.flatten() {
            out.push(format!("  {}", e.file_name().to_string_lossy()));
            // rider/ holds helmets/, boots/... with the actual mods one level down
            if c == "rider" {
                for sub in fs::read_dir(e.path()).into_iter().flatten().flatten() {
                    out.push(format!("    {}", sub.file_name().to_string_lossy()));
                }
            }
        }
    }
    Ok(out)
}

/// "tracks/JS7 Compound", "bikes/Foo.pkz", "rider/helmets/Bar" instead of every file.
fn summary(plan: &Plan) -> BTreeSet<String> {
    plan.iter()
        .map(|(_, d)| {
            let depth = if d.starts_with("rider") { 3 } else { 2 };
            let parts: Vec<_> = d.components().take(depth).map(|c| c.as_os_str().to_string_lossy()).collect();
            parts.join("/")
        })
        .collect()
}

/// Breadth-first, so the shallowest folders come first.
fn walk(root: &Path) -> R<(Vec<PathBuf>, Vec<PathBuf>)> {
    let (mut dirs, mut files, mut i) = (vec![root.to_path_buf()], vec![], 0);
    while i < dirs.len() {
        for e in fs::read_dir(&dirs[i])? {
            let p = e?.path();
            if p.is_dir() { dirs.push(p) } else { files.push(p) }
        }
        i += 1;
    }
    Ok((dirs, files))
}

fn tar(args: &[&OsStr]) -> R<String> {
    // Must be bsdtar: GNU tar (Git for Windows, plain `tar` on SteamOS) can't read zip/rar/7z.
    // On Windows use the full path, since Git's tar is often first on PATH.
    let exe = if cfg!(windows) {
        PathBuf::from(env::var_os("SystemRoot").unwrap_or("C:\\Windows".into())).join(r"System32\tar.exe")
    } else {
        PathBuf::from("bsdtar")
    };
    let out = Command::new(&exe).args(args).output()
        .map_err(|e| format!("can't run {} ({e})", exe.display()))?;
    if !out.status.success() {
        return Err(format!("tar: {}", String::from_utf8_lossy(&out.stderr).trim()).into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn has_ext(p: &Path, ext: &str) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn stem(p: &Path) -> String {
    p.file_stem().unwrap_or_default().to_string_lossy().into_owned()
}

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str, files: &[&str]) -> TempDir {
        let root = TempDir(env::temp_dir().join(format!("mxb-mods-test-{name}-{}", std::process::id())));
        for f in files {
            let p = root.0.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "").unwrap();
        }
        root
    }

    fn dests(root: &Path, name: &str) -> Vec<String> {
        let mut d: Vec<_> = plan(root, name).unwrap().into_iter()
            .map(|(_, d)| d.to_string_lossy().replace('\\', "/")).collect();
        d.sort();
        d
    }

    #[test]
    fn plans() {
        // The JS7 Compound bug: no top folder, layouts as subfolders. Must stay in one track folder.
        let flat = tree("flat", &["JS7 Compound - b4.map", "clearsky.edf", "Nat/Nat.trh", "readme.txt"]);
        assert_eq!(dests(&flat.0, "JS7 Compound"), [
            "tracks/JS7 Compound/JS7 Compound - b4.map",
            "tracks/JS7 Compound/Nat/Nat.trh",
            "tracks/JS7 Compound/clearsky.edf",
            "tracks/JS7 Compound/readme.txt",
        ]);

        // Wrapped in a top folder (and a readme outside it): folder name wins, readme skipped.
        let nested = tree("nested", &["Read me.txt", "Club MX/Club MX.map", "Club MX/sky.edf"]);
        assert_eq!(dests(&nested.0, "clubmx_v2"), ["tracks/Club MX/Club MX.map", "tracks/Club MX/sky.edf"]);

        // Already laid out like the mods folder, with a stray file at the top.
        let laid = tree("laid", &["x/mods/rider/helmets/Bell/Bell.edf", "x/mods/tracks/A/A.map", "x/notes.txt"]);
        assert_eq!(dests(&laid.0, "pack"), ["rider/helmets/Bell/Bell.edf", "tracks/A/A.map"]);

        // Track pack: two tracks side by side.
        let pack = tree("pack", &["A/A.map", "B/B.map", "B/Alt/Alt.trh"]);
        assert_eq!(dests(&pack.0, "pack"), ["tracks/A/A.map", "tracks/B/Alt/Alt.trh", "tracks/B/B.map"]);

        // Made on Windows with capitalised folders: still lands in lowercase tracks/ (matters on Linux).
        let caps = tree("caps", &["Mods/Tracks/A/A.map", "Mods/Rider/helmets/H/H.edf"]);
        assert_eq!(dests(&caps.0, "caps"), ["rider/helmets/H/H.edf", "tracks/A/A.map"]);

        // Nothing recognisable.
        let junk = tree("junk", &["photo.png"]);
        assert!(plan(&junk.0, "junk").is_err());
    }

    #[test]
    fn steam_libraries() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/deck/.local/share/Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"/run/media/mmcblk0p1\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"655500\"\t\t\"12345\"\n\t\t}\n\t}\n}\n";
        assert_eq!(library_paths(vdf), [PathBuf::from("/home/deck/.local/share/Steam"), PathBuf::from("/run/media/mmcblk0p1")]);
    }
}
