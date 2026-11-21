use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use image::{Rgb, RgbImage};
use serde_json::json;

use crate::plugin::PLUGIN_DIR_NAME;

pub const FLATPAK_ID: &str = "me.amankhanna.opendeck";

pub fn plugin_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })
    {
        roots.push(config.join("opendeck").join("plugins"));
    }
    if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        roots.push(
            PathBuf::from(home)
                .join(".var/app")
                .join(FLATPAK_ID)
                .join("config/opendeck/plugins"),
        );
    }
    roots
}

pub fn default_plugin_root() -> PathBuf {
    let roots = plugin_roots();
    if let Some(native) = roots.first()
        && native.exists()
    {
        return native.clone();
    }
    if let Some(flatpak) = roots.get(1) {
        if flatpak.exists() {
            return flatpak.clone();
        }
        let app_dir = flatpak
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent);
        if app_dir.is_some_and(Path::exists) {
            return flatpak.clone();
        }
    }
    roots
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from(".").join("plugins"))
}

pub fn install(root: Option<PathBuf>) -> Result<PathBuf> {
    let root = root.unwrap_or_else(default_plugin_root);
    let dir = if root.extension().is_some_and(|ext| ext == "sdPlugin") {
        root
    } else {
        root.join(PLUGIN_DIR_NAME)
    };
    fs::create_dir_all(dir.join("icons"))
        .with_context(|| format!("cannot create {}", dir.display()))?;

    let exe = std::env::current_exe().context("cannot locate the running binary")?;
    let target = dir.join("opendeck-strip");
    if exe != target {
        // Rename over the old binary; a running plugin would make an in-place
        // copy fail with ETXTBSY, while rename replaces the directory entry.
        let staging = dir.join(format!(".opendeck-strip.{}.new", std::process::id()));
        fs::copy(&exe, &staging)
            .inspect_err(|_| {
                let _ = fs::remove_file(&staging);
            })
            .with_context(|| format!("cannot copy {} to {}", exe.display(), staging.display()))?;
        fs::rename(&staging, &target)
            .inspect_err(|_| {
                let _ = fs::remove_file(&staging);
            })
            .with_context(|| format!("cannot replace {}", target.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
    }

    write_icon(&dir.join("icons/plugin.png"))?;
    write_icon(&dir.join("icons/category.png"))?;
    write_icon(&dir.join("icons/action.png"))?;

    let manifest = json!({
        "Name": "Status Display",
        "Description": "Live status text on keys, dials and the N4E touch strip",
        "Author": "local",
        "Version": env!("CARGO_PKG_VERSION"),
        "PluginUUID": "local.statusdisplay",
        "Icon": "icons/plugin",
        "Category": "Status Display",
        "CategoryIcon": "icons/category",
        "OS": [{"Platform": "linux"}],
        "CodePath": "opendeck-strip",
        "CodePathLin": "opendeck-strip",
        "Actions": [{
            "Name": "Status Slot",
            "UUID": crate::plugin::ACTION_UUID,
            "Icon": "icons/action",
            "Controllers": ["Keypad", "Encoder"],
            "States": [{"Image": "icons/action", "Name": "Status"}],
            "Encoder": {
                "layout": "$A0",
                "Icon": "icons/action"
            }
        }]
    });
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest)?,
    )
    .context("cannot write manifest.json")?;

    Ok(dir)
}

pub fn installed_dir(root: &Path) -> PathBuf {
    root.join(PLUGIN_DIR_NAME)
}

fn write_icon(path: &Path) -> Result<()> {
    let size = 256u32;
    let mut image = RgbImage::new(size, size);
    for y in 0..size {
        for x in 0..size {
            let cx = x as f32 / size as f32;
            let cy = y as f32 / size as f32;
            let edge = 16.0 / size as f32;
            let inside = cx > edge && cx < 1.0 - edge && cy > edge && cy < 1.0 - edge;
            let color = if inside {
                let v = (18.0 + 18.0 * (1.0 - cy)) as u8;
                Rgb([v / 2, v, v.saturating_add(30)])
            } else {
                Rgb([8, 10, 14])
            };
            image.put_pixel(x, y, color);
        }
    }
    // A wide strip and three key blocks.
    for y in size * 42 / 100..size * 58 / 100 {
        for x in size * 16 / 100..size * 84 / 100 {
            image.put_pixel(x, y, Rgb([80, 220, 255]));
        }
    }
    for row in 0..2u32 {
        for col in 0..3u32 {
            let x0 = size * (16 + col * 24) / 100;
            let y0 = size * (66 + row * 10) / 100;
            for y in y0..y0 + size * 6 / 100 {
                for x in x0..x0 + size * 16 / 100 {
                    image.put_pixel(x, y, Rgb([235, 240, 245]));
                }
            }
        }
    }
    image
        .save(path)
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

pub fn read_installed_manifest(root: &Path) -> Result<serde_json::Value> {
    let path = installed_dir(root).join("manifest.json");
    if !path.exists() {
        bail!("not installed in {}", root.display());
    }
    let text = fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&text)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_writes_manifest_and_icons() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("plugins");
        let installed = install(Some(root.clone())).unwrap();
        assert_eq!(installed, root.join(PLUGIN_DIR_NAME));
        assert!(installed.join("manifest.json").exists());
        assert!(installed.join("icons/plugin.png").exists());
        assert!(installed.join("opendeck-strip").exists());
        let manifest = read_installed_manifest(&root).unwrap();
        assert_eq!(manifest["Actions"][0]["UUID"], crate::plugin::ACTION_UUID);
        assert_eq!(manifest["Actions"][0]["Encoder"]["layout"], "$A0");
    }
}
