; Instalador de Form PDF Reader para Windows (Inno Setup 6).
;
;   iscc /DAppVersion=1.0.0 /DSrcDir=..\..\dist\windows packaging\windows\form-pdf-reader.iss
;
; SrcDir: carpeta con form-pdf-reader.exe, pdfium.dll, LICENSE.txt y
; THIRD-PARTY-NOTICES.txt (la misma que la versión portable).
; Se instala para el usuario sin pedir permisos de administrador (o para
; todos, si se elige en el asistente). Se ofrece en "Abrir con" para los PDF
; y, si se marca, se abre la configuración de Windows para elegirla como
; predeterminada (Windows no deja cambiarla sin que lo confirme el usuario).
;
; La aplicación se actualiza sola lanzando este instalador con
;   /SILENT /SP- /SUPPRESSMSGBOXES /NORESTART /UPDATE=1
; y /UPDATE=1 hace que se vuelva a abrir al terminar.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SrcDir
  #define SrcDir "..\..\dist\windows"
#endif
#define AppName "Form PDF Reader"
#define AppExe "form-pdf-reader.exe"

[Setup]
AppId={{61B2DD7A-5D1C-4FFE-BDA6-F9528A69AED9}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=blzkz
AppPublisherURL=https://github.com/blzkz/form-pdf-reader
AppSupportURL=https://github.com/blzkz/form-pdf-reader/issues
AppUpdatesURL=https://github.com/blzkz/form-pdf-reader/releases
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\dist
OutputBaseFilename=form-pdf-reader-{#AppVersion}-windows-x64-setup
SetupIconFile=form-pdf-reader.ico
UninstallDisplayIcon={app}\{#AppExe}
LicenseFile=..\..\LICENSE
WizardStyle=modern
Compression=lzma2
SolidCompression=yes
ChangesAssociations=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "french"; MessagesFile: "compiler:Languages\French.isl"
Name: "italian"; MessagesFile: "compiler:Languages\Italian.isl"
Name: "portuguese"; MessagesFile: "compiler:Languages\Portuguese.isl"
Name: "german"; MessagesFile: "compiler:Languages\German.isl"

[CustomMessages]
english.DefaultPdf=Open PDF files with {#AppName} by default (Windows settings will open to confirm it)
spanish.DefaultPdf=Abrir los PDF con {#AppName} por defecto (se abrirá la configuración de Windows para confirmarlo)
french.DefaultPdf=Ouvrir les PDF avec {#AppName} par défaut (les paramètres de Windows s'ouvriront pour le confirmer)
italian.DefaultPdf=Aprire i PDF con {#AppName} per impostazione predefinita (si apriranno le impostazioni di Windows per confermarlo)
portuguese.DefaultPdf=Abrir os PDF com o {#AppName} por predefinição (as definições do Windows abrirão para o confirmar)
german.DefaultPdf=PDF-Dateien standardmäßig mit {#AppName} öffnen (die Windows-Einstellungen werden zum Bestätigen geöffnet)
english.Associations=File associations:
spanish.Associations=Asociación de ficheros:
french.Associations=Association de fichiers :
italian.Associations=Associazione dei file:
portuguese.Associations=Associação de ficheiros:
german.Associations=Dateizuordnung:

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
; Solo si no es ya la aplicación de los PDF.
Name: "defaultpdf"; Description: "{cm:DefaultPdf}"; GroupDescription: "{cm:Associations}"; Check: NotDefaultPdf

[Files]
Source: "{#SrcDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SrcDir}\pdfium.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SrcDir}\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SrcDir}\THIRD-PARTY-NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; "Abrir con" para los PDF, sin cambiar la aplicación predeterminada.
Root: HKA; Subkey: "Software\Classes\FormPdfReader.pdf"; ValueType: string; ValueData: "PDF"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\FormPdfReader.pdf\DefaultIcon"; ValueType: string; ValueData: "{app}\{#AppExe},0"
Root: HKA; Subkey: "Software\Classes\FormPdfReader.pdf\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""
Root: HKA; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: string; ValueName: "FormPdfReader.pdf"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "{#AppName}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".pdf"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""
; Para que salga en "Aplicaciones predeterminadas" de Windows.
Root: HKA; Subkey: "Software\FormPdfReader"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\FormPdfReader\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "{#AppName}"
Root: HKA; Subkey: "Software\FormPdfReader\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "{#AppName}"
Root: HKA; Subkey: "Software\FormPdfReader\Capabilities"; ValueType: string; ValueName: "ApplicationIcon"; ValueData: "{app}\{#AppExe},0"
Root: HKA; Subkey: "Software\FormPdfReader\Capabilities\FileAssociations"; ValueType: string; ValueName: ".pdf"; ValueData: "FormPdfReader.pdf"
Root: HKA; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "FormPdfReader"; ValueData: "Software\FormPdfReader\Capabilities"; Flags: uninsdeletevalue

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
; La página de la aplicación en "Aplicaciones predeterminadas" (Windows 11;
; en Windows 10, la página general).
Filename: "ms-settings:defaultapps?{code:RegisteredApp}=FormPdfReader"; Flags: shellexec nowait skipifsilent runasoriginaluser; Tasks: defaultpdf
; Actualización desde la propia aplicación: volver a abrirla.
Filename: "{app}\{#AppExe}"; Flags: nowait runasoriginaluser; Check: IsUpdate

[Code]
function IsUpdate: Boolean;
begin
  Result := ExpandConstant('{param:UPDATE|0}') = '1';
end;

function NotDefaultPdf: Boolean;
var
  ProgId: String;
begin
  Result := True;
  if RegQueryStringValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.pdf\UserChoice', 'ProgId', ProgId) then
    Result := (ProgId <> 'FormPdfReader.pdf') and (ProgId <> 'Applications\{#AppExe}');
end;

function RegisteredApp(Param: String): String;
begin
  if IsAdminInstallMode then
    Result := 'registeredAppMachine'
  else
    Result := 'registeredAppUser';
end;
