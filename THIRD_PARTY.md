# Third-party notices

WallpaperLive's interface and desktop controller are written in Rust. The application uses the following components; their names and licenses do not imply endorsement of WallpaperLive.

## Rust dependencies

- `windows` and its supporting crates: MIT OR Apache-2.0, Copyright (c) Microsoft Corporation. The MIT option is reproduced below.
- `embed-manifest`: MIT, Copyright (c) 2021, 2022, 2025, 2026 Carey Evans.
- `proc-macro2`, `quote`, `syn`, and `unicode-ident`: MIT OR Apache-2.0; Unicode tables also carry the Unicode License V3.

Exact versions are recorded in `Cargo.lock`. Build tools are not installed with the application.

### MIT permission notice

Copyright (c) Microsoft Corporation.
Copyright (c) 2021, 2022, 2025, 2026 Carey Evans. All rights reserved.

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

### Unicode License V3

Copyright © 1991-2023 Unicode, Inc.

NOTICE TO USER: Carefully read the following legal agreement. BY DOWNLOADING, INSTALLING, COPYING OR OTHERWISE USING DATA FILES, AND/OR SOFTWARE, YOU UNEQUIVOCALLY ACCEPT, AND AGREE TO BE BOUND BY, ALL OF THE TERMS AND CONDITIONS OF THIS AGREEMENT. IF YOU DO NOT AGREE, DO NOT DOWNLOAD, INSTALL, COPY, DISTRIBUTE OR USE THE DATA FILES OR SOFTWARE.

Permission is hereby granted, free of charge, to any person obtaining a copy of data files and any associated documentation (the "Data Files") or software and any associated documentation (the "Software") to deal in the Data Files or Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, and/or sell copies of the Data Files or Software, and to permit persons to whom the Data Files or Software are furnished to do so, provided that either (a) this copyright and permission notice appear with all copies of the Data Files or Software, or (b) this copyright and permission notice appear in associated Documentation.

THE DATA FILES AND SOFTWARE ARE PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF THIRD PARTY RIGHTS.

IN NO EVENT SHALL THE COPYRIGHT HOLDER OR HOLDERS INCLUDED IN THIS NOTICE BE LIABLE FOR ANY CLAIM, OR ANY SPECIAL INDIRECT OR CONSEQUENTIAL DAMAGES, OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THE DATA FILES OR SOFTWARE.

Except as contained in this notice, the name of a copyright holder shall not be used in advertising or otherwise to promote the sale, use or other dealings in these Data Files or Software without prior written authorization of the copyright holder.

## Native media decoder

Playback and local cache encoding use libmpv, a native C library. Cache encoding invokes the H.264/AAC encoders already present in the user's installed decoder; no additional transcoder executable or decoder bytes are bundled. Setup verifies and reuses an installed decoder or a user-provided pinned archive named `mpv-dev.7z` beside the installer. If neither is available, it obtains the pinned Windows build directly from its publisher after explicit consent. Neither the decoder archive nor DLL is included in WallpaperLive's source or installer.

Selected decoder: `mpv-dev-x86_64-20261107-git-9028187538.7z`.

```text
Archive SHA256: df7c433a2d481fdaf6163b201f26abbb0eff0c215c51368397088c6d8f7a001b
DLL SHA256:     c91949577ae9cc2d500208da51f2c4a24cfa7a02d0ce6e130bf67ec738b450bb
```

mpv is GPL-2.0-or-later by default. Its client API header is ISC-licensed, which does not relicense the decoder. Linked codecs and libraries have their own licenses. The full corresponding-source record of this third-party Windows build has not been audited for redistribution, so it must not be bundled or republished as part of this release. The application accepts compatible user-supplied libmpv API 2 libraries from its documented local locations.

## Installer components

Inno Setup: Copyright (C) 1997-2026 Jordan Russell; portions Copyright (C) 2000-2026 Martijn Laan.

Native archive extraction uses 7-Zip/LZMA components with their upstream notices retained in the installer engine. No compiler program is installed on the user's machine. Inno Setup 7.1.0 used for this build reports non-commercial use; review its licensing before commercial distribution. The produced WallpaperLive installer is unsigned.

## Application license

WallpaperLive is GPL-3.0-or-later. Preserve application and dependency notices and provide corresponding source when distributing the application. Wallpaper media is not included and remains subject to its owner's rights.
