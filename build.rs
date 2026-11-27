use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn latest_rc_in_kit_bin(bin_root: &Path) -> Option<PathBuf> {
    let mut versions: Vec<_> = fs::read_dir(bin_root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("x64/rc.exe"))
        .filter(|path| path.is_file())
        .collect();
    versions.sort();
    versions.pop()
}

fn find_rc_compiler() -> PathBuf {
    for var in ["RC", "RC_PATH"] {
        if let Some(path) = env::var_os(var).map(PathBuf::from)
            && path.is_file()
        {
            return path;
        }
    }
    if let Some(ver_bin) = env::var_os("WindowsSdkVerBinPath").map(PathBuf::from) {
        let candidate = ver_bin.join("x64/rc.exe");
        if candidate.is_file() {
            return candidate;
        }
    }
    if let Some(sdk_dir) = env::var_os("WindowsSdkDir").map(PathBuf::from)
        && let Some(path) = latest_rc_in_kit_bin(&sdk_dir.join("bin"))
    {
        return path;
    }
    if let Some(pf86) = env::var_os("ProgramFiles(x86)").map(PathBuf::from)
        && let Some(path) = latest_rc_in_kit_bin(&pf86.join("Windows Kits/10/bin"))
    {
        return path;
    }
    panic!(
        "Windows SDK rc.exe not found (checked RC, RC_PATH, WindowsSdkVerBinPath, WindowsSdkDir, and Windows Kits/10/bin)"
    );
}

fn rc_escape_path(path: &Path) -> String {
    path.to_str()
        .expect("Windows build path must be valid UTF-8")
        .replace('\\', "\\\\")
}

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=assets/wallpaperlive.ico");
    println!("cargo:rerun-if-env-changed=RC");
    println!("cargo:rerun-if-env-changed=RC_PATH");
    println!("cargo:rerun-if-env-changed=WindowsSdkVerBinPath");
    println!("cargo:rerun-if-env-changed=WindowsSdkDir");

    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // Embed icon, manifest, and VS_VERSION_INFO at link time via rc.exe so post-link
        // UpdateResourceW never shifts .reloc and breaks /GUARD:CF + /CETCOMPAT (0xC0000142).
        let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("MANIFEST_DIR"));
        let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
        let pkg_ver = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
        let mut nums = pkg_ver.split('.').map(|s| s.parse::<u16>().unwrap_or(0));
        let major = nums.next().unwrap_or(0);
        let minor = nums.next().unwrap_or(0);
        let patch = nums.next().unwrap_or(0);
        let product_ver = format!("{major}.{minor}.{patch}");
        let file_ver = format!("{major}.{minor}.{patch}.0");

        let icon_path = rc_escape_path(&manifest_dir.join("assets").join("wallpaperlive.ico"));
        let manifest_path = rc_escape_path(&manifest_dir.join("app.manifest"));
        let rc_source = format!(
            concat!(
                "LANGUAGE 9, 1\r\n",
                "1 ICON \"{icon}\"\r\n",
                "1 24 \"{manifest}\"\r\n",
                "1 VERSIONINFO\r\n",
                "FILEVERSION {major},{minor},{patch},0\r\n",
                "PRODUCTVERSION {major},{minor},{patch},0\r\n",
                "FILEFLAGSMASK 0x3fL\r\n",
                "FILEFLAGS 0\r\n",
                "FILEOS 0x40004L\r\n",
                "FILETYPE 1\r\n",
                "BEGIN\r\n",
                "  BLOCK \"StringFileInfo\"\r\n",
                "  BEGIN\r\n",
                "    BLOCK \"040904b0\"\r\n",
                "    BEGIN\r\n",
                "      VALUE \"CompanyName\", \"Raflyf\\0\"\r\n",
                "      VALUE \"FileDescription\", \"WallpaperLive\\0\"\r\n",
                "      VALUE \"FileVersion\", \"{file_ver}\\0\"\r\n",
                "      VALUE \"InternalName\", \"wallpaperlive\\0\"\r\n",
                "      VALUE \"LegalCopyright\", \"GPL-3.0-or-later\\0\"\r\n",
                "      VALUE \"OriginalFilename\", \"wallpaperlive.exe\\0\"\r\n",
                "      VALUE \"ProductName\", \"WallpaperLive\\0\"\r\n",
                "      VALUE \"ProductVersion\", \"{product_ver}\\0\"\r\n",
                "    END\r\n",
                "  END\r\n",
                "  BLOCK \"VarFileInfo\"\r\n",
                "  BEGIN\r\n",
                "    VALUE \"Translation\", 0x0409, 1200\r\n",
                "  END\r\n",
                "END\r\n"
            ),
            icon = icon_path,
            manifest = manifest_path,
            major = major,
            minor = minor,
            patch = patch,
            file_ver = file_ver,
            product_ver = product_ver,
        );

        let rc_file = out_dir.join("wallpaperlive.rc");
        let res_file = out_dir.join("wallpaperlive.res");
        fs::write(&rc_file, rc_source).expect("write generated wallpaperlive.rc");

        let rc = find_rc_compiler();
        let status = Command::new(&rc)
            .arg("/nologo")
            .arg("/fo")
            .arg(&res_file)
            .arg(&rc_file)
            .status()
            .expect("run rc.exe");
        assert!(status.success(), "rc.exe failed for wallpaperlive.rc");

        println!("cargo:rustc-link-arg=/DEPENDENTLOADFLAG:0x800");
        println!("cargo:rustc-link-arg=/CETCOMPAT");
        println!("cargo:rustc-link-arg=/GUARD:CF");
        println!(
            "cargo:rustc-link-arg-bin=wallpaperlive={}",
            res_file.display()
        );
        println!("cargo:rustc-link-arg-bin=wallpaperlive=/MANIFEST:NO");
    } else {
        embed_manifest::embed_manifest_file("app.manifest")
            .expect("Windows manifest must embed successfully");
    }
}
