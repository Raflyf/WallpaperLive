#ifndef AppExe
  #define AppExe "..\target\release\wallpaperlive.exe"
#endif
#define AppVersion "0.2.1"
#define DecoderURL "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261107/mpv-dev-x86_64-20261107-git-9028187538.7z"
#define ArchiveSHA256 "df7c433a2d481fdaf6163b201f26abbb0eff0c215c51368397088c6d8f7a001b"
#define DecoderSHA256 "c91949577ae9cc2d500208da51f2c4a24cfa7a02d0ce6e130bf67ec738b450bb"

[Setup]
AppId={{F92A66DF-7B95-40A7-934B-0A1785996391}
AppName=WallpaperLive
AppVersion={#AppVersion}
VersionInfoVersion={#AppVersion}.0
VersionInfoProductName=WallpaperLive
VersionInfoProductVersion={#AppVersion}
VersionInfoCompany=Raflyf
VersionInfoDescription=WallpaperLive Setup
VersionInfoCopyright=GPL-3.0-or-later
AppPublisher=Raflyf
AppPublisherURL=https://github.com/Raflyf/WallpaperLive
DefaultDirName={localappdata}\Programs\WallpaperLive
DefaultGroupName=WallpaperLive
PrivilegesRequired=lowest
SetupArchitecture=x64
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
AppMutex=Local\Raflyf.WallpaperLive
SetupMutex=Local\Raflyf.WallpaperLive.Setup
CloseApplications=yes
CloseApplicationsFilter=*wallpaperlive.exe
RestartApplications=no
AlwaysRestart=no
SetupIconFile=..\assets\wallpaperlive.ico
UninstallDisplayIcon={app}\wallpaperlive.ico
LicenseFile=..\LICENSE
InfoBeforeFile=ONLINE-NOTICE.txt
DisableProgramGroupPage=yes
WizardStyle=modern
OutputDir=..\dist
OutputBaseFilename=wallpaperlivesetup-v{#AppVersion}
Compression=lzma2
SolidCompression=yes
ArchiveExtraction=enhanced/nopassword
ExtraDiskSpaceRequired=300000000

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\THIRD_PARTY.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\wallpaperlive.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "{tmp}\decoder\libmpv-2.dll"; DestDir: "{app}\runtime"; ExternalSize: 121624576; Hash: "{#DecoderSHA256}"; Flags: external ignoreversion onlyifdoesntexist; Check: NeedDecoder

[Icons]
Name: "{userprograms}\WallpaperLive"; Filename: "{app}\wallpaperlive.exe"; WorkingDir: "{app}"; IconFilename: "{app}\wallpaperlive.ico"; AppUserModelID: "Raflyf.WallpaperLive"

[Run]
Filename: "{app}\wallpaperlive.exe"; Description: "{cm:LaunchProgram,WallpaperLive}"; Flags: nowait postinstall skipifsilent

[Code]
const
  { Must stay in sync with ACTION (WM_APP + 1) and EXIT in src/app.rs. }
  WM_WALLPAPERLIVE_ACTION = $8001;
  ID_WALLPAPERLIVE_EXIT = 110;

var
  ConsentPage: TInputOptionWizardPage;
  DownloadPage: TDownloadWizardPage;
  DecoderPrepared: Boolean;
  DownloadDecoder: Boolean;

function NeedDecoder: Boolean;
begin
  Result := DownloadDecoder;
end;

procedure CloseRunningApplication;
var
  Wnd: HWND;
  Attempts: Integer;
  Posted: Boolean;
begin
  Posted := False;
  for Attempts := 0 to 99 do
  begin
    if not CheckForMutexes('Local\Raflyf.WallpaperLive') then Exit;
    if not Posted then
    begin
      Wnd := FindWindowByClassName('WallpaperLive.Panel');
      if Wnd <> 0 then
      begin
        PostMessage(Wnd, WM_WALLPAPERLIVE_ACTION, ID_WALLPAPERLIVE_EXIT, 0);
        Posted := True;
      end;
    end;
    Sleep(50);
  end;
end;

function InitializeSetup: Boolean;
begin
  Result := True;
  CloseRunningApplication;
end;

function InitializeUninstall: Boolean;
begin
  Result := True;
  CloseRunningApplication;
end;

procedure InitializeWizard;
begin
  ConsentPage := CreateInputOptionPage(wpSelectDir,
    'Native decoder download',
    'Internet access is required once to prepare playback.',
    'Setup downloads approximately 31.8 MB from GitHub (shinchiro/mpv-winbuild-cmake), verifies SHA256, and installs the native decoder. No wallpaper, startup entry, or application is launched. Cancel Setup if you do not want this download.',
    False, False);
  ConsentPage.Add('I agree to download the pinned native decoder when required.');
  ConsentPage.Values[0] := False;
  DownloadPage := CreateDownloadPage('Preparing native decoder',
    'Downloading and verifying the pinned upstream build.', nil);
  DownloadPage.ShowBaseNameInsteadOfUrl := True;
end;

function LocalDecoderAvailable: Boolean;
begin
  Result := FileExists(ExpandConstant('{app}\runtime\libmpv-2.dll')) or
    FileExists(ExpandConstant('{src}\mpv-dev.7z'));
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := (PageID = ConsentPage.ID) and LocalDecoderAvailable;
end;

function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := True;
  if (CurPageID = ConsentPage.ID) and not LocalDecoderAvailable then
    if not ConsentPage.Values[0] then begin
      SuppressibleMsgBox('Decoder consent is required. Cancel Setup if you do not want this download.', mbInformation, MB_OK, IDOK);
      Result := False;
    end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ExistingDecoder: String;
  DecoderDir: String;
  DecoderArchive: String;
begin
  Result := '';
  NeedsRestart := False;
  CloseRunningApplication;
  if not LocalDecoderAvailable and not ConsentPage.Values[0] then begin
    Result := 'Decoder download consent was not granted. Nothing installed.';
    Exit;
  end;
  try
    ExistingDecoder := ExpandConstant('{app}\runtime\libmpv-2.dll');
    if FileExists(ExistingDecoder) then begin
      if CompareText(GetSHA256OfFile(ExistingDecoder), '{#DecoderSHA256}') <> 0 then
        RaiseException('Existing decoder differs from the pinned build and will not be overwritten. Choose another installation directory.');
      DownloadDecoder := False;
      Exit;
    end;
    DownloadDecoder := True;
    if DecoderPrepared then Exit;
    DecoderArchive := ExpandConstant('{src}\mpv-dev.7z');
    if FileExists(DecoderArchive) then begin
      if CompareText(GetSHA256OfFile(DecoderArchive), '{#ArchiveSHA256}') <> 0 then
        RaiseException('Local mpv-dev.7z checksum differs from the pinned decoder. Nothing installed; move that file aside or supply the correct upstream archive.');
    end else begin
      if not ConsentPage.Values[0] then
        RaiseException('Decoder is not available locally and download consent was not granted.');
      DownloadPage.Clear;
      DownloadPage.Add('{#DecoderURL}', 'mpv-dev.7z', '{#ArchiveSHA256}');
      DownloadPage.Show;
      try
        DownloadPage.Download;
      finally
        DownloadPage.Hide;
      end;
      DecoderArchive := ExpandConstant('{tmp}\mpv-dev.7z');
    end;
    DecoderDir := ExpandConstant('{tmp}\decoder');
    if not ForceDirectories(DecoderDir) then RaiseException('Could not create temporary decoder directory.');
    ExtractArchive(DecoderArchive, DecoderDir, '', True, nil);
    if not FileExists(DecoderDir + '\libmpv-2.dll') then RaiseException('Decoder DLL is missing from the verified archive.');
    if CompareText(GetSHA256OfFile(DecoderDir + '\libmpv-2.dll'), '{#DecoderSHA256}') <> 0 then
      RaiseException('Extracted decoder SHA256 mismatch. Nothing installed.');
    DecoderPrepared := True;
  except
    Result := 'Decoder preparation failed. Retry with Internet access or cancel Setup. ' + GetExceptionMessage;
  end;
end;
