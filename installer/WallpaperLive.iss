#ifndef AppExe
  #define AppExe "..\target\release\wallpaperlive.exe"
#endif
#define AppVersion "0.1.5"
#define DecoderURL "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261107/mpv-dev-x86_64-20261107-git-9028187538.7z"
#define ArchiveSHA256 "df7c433a2d481fdaf6163b201f26abbb0eff0c215c51368397088c6d8f7a001b"
#define DecoderSHA256 "c91949577ae9cc2d500208da51f2c4a24cfa7a02d0ce6e130bf67ec738b450bb"

[Setup]
AppId={{F92A66DF-7B95-40A7-934B-0A1785996391}
AppName=WallpaperLive
AppVersion={#AppVersion}
VersionInfoVersion={#AppVersion}.0
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
Name: "{userprograms}\WallpaperLive"; Filename: "{app}\wallpaperlive.exe"; WorkingDir: "{app}"; IconFilename: "{app}\wallpaperlive.ico"

[Run]
Filename: "{app}\wallpaperlive.exe"; Description: "{cm:LaunchProgram,WallpaperLive}"; Flags: nowait postinstall skipifsilent

[Code]
var
  DecoderInfoPage: TOutputMsgWizardPage;
  DownloadPage: TDownloadWizardPage;
  ProgressPage: TOutputMarqueeProgressWizardPage;
  DecoderPrepared: Boolean;
  DownloadDecoder: Boolean;

function NeedDecoder: Boolean;
begin
  Result := DownloadDecoder;
end;

function InitializeSetup: Boolean;
var
  ResultCode: Integer;
begin
  Result := True;
  // Automatically terminate running wallpaperlive.exe so user does not have to exit from tray manually
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/f /im wallpaperlive.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Sleep(250);
end;

function InitializeUninstall: Boolean;
var
  ResultCode: Integer;
begin
  Result := True;
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/f /im wallpaperlive.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Sleep(250);
end;

procedure InitializeWizard;
begin
  DecoderInfoPage := CreateOutputMsgPage(wpSelectDir,
    'Native decoder download',
    'Internet access is required once to prepare playback.',
    'Setup will download approximately 31.8 MB from GitHub (shinchiro/mpv-winbuild-cmake), verify SHA256 integrity, and install the native playback decoder.' + #13#10#13#10 +
    'Click Next to continue and download the decoder, or Cancel to exit Setup.');
  DownloadPage := CreateDownloadPage('Preparing native decoder',
    'Downloading and verifying the pinned upstream build.', nil);
  DownloadPage.ShowBaseNameInsteadOfUrl := True;
  ProgressPage := CreateOutputMarqueeProgressPage('Preparing native decoder',
    'Downloading and verifying the pinned playback decoder (about 31.8 MB)...');
end;

function LocalDecoderAvailable: Boolean;
begin
  Result := FileExists(ExpandConstant('{app}\runtime\libmpv-2.dll')) or
    FileExists(ExpandConstant('{src}\mpv-dev.7z'));
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := (PageID = DecoderInfoPage.ID) and LocalDecoderAvailable;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ExistingDecoder: String;
  DecoderDir: String;
  DecoderArchive: String;
  ResultCode: Integer;
begin
  Result := '';
  NeedsRestart := False;
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/f /im wallpaperlive.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Sleep(150);
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
      DecoderArchive := ExpandConstant('{tmp}\mpv-dev.7z');
      ProgressPage.SetText('Downloading decoder from GitHub (about 31.8 MB)...', 'Please wait, this is only downloaded once.');
      ProgressPage.Animate;
      ProgressPage.Show;
      try
        // Use system curl with browser User-Agent directly to connect in 0.5s without 30s GitHub timeout
        if FileExists(ExpandConstant('{sys}\curl.exe')) then begin
          Exec(ExpandConstant('{sys}\curl.exe'), '-f -s -S -L -A "Mozilla/5.0" -o "' + DecoderArchive + '" "{#DecoderURL}"', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
        end;
        if not FileExists(DecoderArchive) or (CompareText(GetSHA256OfFile(DecoderArchive), '{#ArchiveSHA256}') <> 0) then begin
          DeleteFile(DecoderArchive);
          Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'), '-NoProfile -NonInteractive -Command "(New-Object System.Net.WebClient).Headers.Add(''User-Agent'',''Mozilla/5.0''); (New-Object System.Net.WebClient).DownloadFile(''{#DecoderURL}'',''' + DecoderArchive + ''')"', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
        end;
        if not FileExists(DecoderArchive) or (CompareText(GetSHA256OfFile(DecoderArchive), '{#ArchiveSHA256}') <> 0) then begin
          DeleteFile(DecoderArchive);
          DownloadPage.Clear;
          DownloadPage.Add('{#DecoderURL}', 'mpv-dev.7z', '{#ArchiveSHA256}');
          DownloadPage.Download;
        end;
      finally
        ProgressPage.Hide;
      end;
      if not FileExists(DecoderArchive) then
        RaiseException('Could not download decoder. Check your Internet connection and retry Setup.');
      if CompareText(GetSHA256OfFile(DecoderArchive), '{#ArchiveSHA256}') <> 0 then
        RaiseException('Downloaded decoder checksum mismatch. Nothing installed.');
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
