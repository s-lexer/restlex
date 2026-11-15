; RestLex offline installer — bundles restlex.exe, restlex-agent.exe, restlex-webserver.exe.
; Build: installer\build-installer.cmd (compiles + EV-signs the output).
#define AppVersion GetFileVersion("..\dist\restlex.exe")

[Setup]
ShowLanguageDialog=auto
AppId={{32A9A138-EEFA-4951-ADBB-23419FC51F12}
AppName=RestLex
AppVersion={#AppVersion}
AppPublisher=RestLex
DefaultDirName={commonappdata}\RestLex
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=admin
Compression=lzma2
SolidCompression=yes
SetupIconFile=..\icon.ico
UninstallDisplayIcon={app}\restlex.exe
OutputDir=..\dist
OutputBaseFilename=RestLex-Setup
WizardStyle=modern

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[CustomMessages]
russian.TypeFull=Полная установка
english.TypeFull=Full installation
russian.TypeCustom=Выборочная установка
english.TypeCustom=Custom installation
russian.CompRestlex=RestLex (агент бэкапов + графический интерфейс)
english.CompRestlex=RestLex (backup agent + GUI)
russian.CompWeb=Веб-сервер отчётов (RestlexWebServer)
english.CompWeb=Report web server (RestlexWebServer)
russian.IconUser=Для текущего пользователя
english.IconUser=For the current user
russian.IconAll=Для всех пользователей
english.IconAll=For all users
russian.IconNone=Не создавать
english.IconNone=Do not create
russian.IconGroup=Ярлык на рабочем столе:
english.IconGroup=Desktop shortcut:
russian.StAclApp=Настройка прав доступа к каталогу установки...
english.StAclApp=Setting permissions on the installation folder...
russian.StAclSecure=Настройка прав доступа к каталогу секретов...
english.StAclSecure=Setting permissions on the secrets folder...
russian.StAgent=Установка службы агента RestlexAgent...
english.StAgent=Installing the RestlexAgent service...
russian.StWeb=Установка службы веб-сервера отчётов...
english.StWeb=Installing the report web server service...
russian.StAclCfg=Ограничение доступа к config.json...
english.StAclCfg=Restricting access to config.json...
russian.StAclProf=Ограничение доступа к profiles.json...
english.StAclProf=Restricting access to profiles.json...
russian.WebPageTitle=Настройка веб-сервера отчётов
english.WebPageTitle=Report web server setup
russian.WebPageSub=Эта страница появляется, только если выбран компонент "Веб-сервер отчётов"
english.WebPageSub=This page appears only if the "Report web server" component is selected
russian.WebPageText=Веб-сервер отчётов — отдельная служба (RestlexWebServer), которая принимает отчёты о бэкапах от агентов и показывает их в браузере на этом компьютере. Придумайте пароль для входа в её веб-интерфейс под логином "admin". Отдельный API-ключ для приёма отчётов от агентов будет сгенерирован автоматически и сохранён в файле api-key.txt в папке установки.
english.WebPageText=The report web server is a separate service (RestlexWebServer) that receives backup reports from agents and shows them in a browser on this computer. Choose a password for its web interface (login "admin"). A separate API key for receiving reports from agents will be generated automatically and saved to api-key.txt in the installation folder.
russian.WebPwd=Пароль администратора веб-интерфейса:
english.WebPwd=Web interface administrator password:
russian.WebPwdEmpty=Введите пароль администратора.
english.WebPwdEmpty=Enter the administrator password.
russian.KeyFileHead=RestLex Web Server — API-ключ для агентов (X-Api-Key):
english.KeyFileHead=RestLex Web Server — API key for agents (X-Api-Key):
russian.KeyFileNote=Файл доступен только администраторам. Удалите его, когда перенесёте ключ на агенты.
english.KeyFileNote=Only administrators can read this file. Delete it after you copy the key to the agents.

[Types]
Name: "full"; Description: "{cm:TypeFull}"
Name: "custom"; Description: "{cm:TypeCustom}"; Flags: iscustom

[Components]
Name: "restlex"; Description: "{cm:CompRestlex}"; Types: full custom
Name: "webserver"; Description: "{cm:CompWeb}"; Types: full custom

[Tasks]
Name: "desktopicon_user"; Description: "{cm:IconUser}"; GroupDescription: "{cm:IconGroup}"; Components: restlex; Flags: exclusive
Name: "desktopicon_all"; Description: "{cm:IconAll}"; GroupDescription: "{cm:IconGroup}"; Components: restlex; Flags: exclusive unchecked
Name: "desktopicon_none"; Description: "{cm:IconNone}"; GroupDescription: "{cm:IconGroup}"; Components: restlex; Flags: exclusive unchecked

[Files]
Source: "..\dist\restlex.exe"; DestDir: "{app}"; Components: restlex; Flags: ignoreversion
Source: "..\dist\restlex-agent.exe"; DestDir: "{app}"; Components: restlex; Flags: ignoreversion
Source: "..\dist\restlex-webserver.exe"; DestDir: "{app}"; Components: webserver; Flags: ignoreversion

[Dirs]
; Подкаталог для секретов. Точечный DACL на config.json/profiles.json стирается
; ПЕРВОЙ ЖЕ записью приложения: и Python `_atomic_write_json`, и Rust
; `write_atomic` пишут `<имя>.tmp` рядом и делают rename, а переименованный файл
; несёт ACL временного файла (унаследованный от каталога, где Users:RX).
; Закрытый ACL здесь наследуется КАЖДЫМ новым файлом, кто бы его ни писал.
; Корень {app} закрыть нельзя: restlex.exe должен читаться ФИЛЬТРОВАННЫМ
; токеном, иначе ярлык не запускается даже у администратора (UAC читает
; манифест до повышения).
Name: "{app}\secure"

[Icons]
Name: "{userdesktop}\RestLex"; Filename: "{app}\restlex.exe"; WorkingDir: "{app}"; Tasks: desktopicon_user
Name: "{commondesktop}\RestLex"; Filename: "{app}\restlex.exe"; WorkingDir: "{app}"; Tasks: desktopicon_all
Name: "{userdesktop}\RestLex Web"; Filename: "http://localhost:8798"; Components: webserver

[Run]
; {app} = C:\ProgramData\RestLex — наследуемые права ProgramData дают Users право
; записи, а служба под SYSTEM грузит отсюда restlex-agent.exe и читает
; config.json/profiles.json. Без этого любой пользователь подменяет бинарь/DLL и
; получает выполнение кода под SYSTEM. SID вместо имён — имена групп локализованы.
; Идёт ПЕРВЫМ, до установки служб. GUI требует прав администратора (uac_admin),
; поэтому Users:RX ему не мешает.
Filename: "{sys}\icacls.exe"; \
    Parameters: """{app}"" /inheritance:r /grant *S-1-5-18:(OI)(CI)F /grant *S-1-5-32-544:(OI)(CI)F /grant *S-1-5-32-545:(OI)(CI)RX"; \
    StatusMsg: "{cm:StAclApp}"; Flags: runhidden waituntilterminated
; Подкаталог для секретов: наследование срезано, Users нет вообще. Ставится ДО
; установки служб, чтобы всё, что туда запишут, уже наследовало закрытый ACL.
Filename: "{sys}\icacls.exe"; \
    Parameters: """{app}\secure"" /inheritance:r /grant *S-1-5-18:(OI)(CI)F /grant *S-1-5-32-544:(OI)(CI)F"; \
    StatusMsg: "{cm:StAclSecure}"; Flags: runhidden waituntilterminated
Filename: "{app}\restlex.exe"; Parameters: "install --all"; \
    Components: restlex; StatusMsg: "{cm:StAgent}"; Flags: runhidden waituntilterminated
; ВНИМАНИЕ: --api-key/--admin-password видны в списке процессов в момент установки.
; Убрать это из инсталлятора в одиночку нельзя: restlex-webserver.exe принимает
; секреты ТОЛЬКО как аргументы (clap, без env/файла). Нужен флаг вида
; --admin-password-file / чтение из stdin на стороне restlex-webserver, после чего
; здесь передавать временный файл с ACL только для SYSTEM+Administrators.
Filename: "{app}\restlex-webserver.exe"; \
    Parameters: "install --api-key {code:GetApiKey} --admin-password ""{code:GetWebPassword}"""; \
    Components: webserver; StatusMsg: "{cm:StWeb}"; Flags: runhidden waituntilterminated
; ПОСЛЕДНИМИ: харденинг config.json/profiles.json. Раньше это делалось в
; CurStepChanged(ssPostInstall) — то есть ДО этой секции, когда на чистой
; установке файлов ещё нет (их создаёт агент при старте службы, а служба
; стартует строкой install --all выше), и icacls просто не вызывался.
; Мера остаётся ПАЛЛИАТИВОМ: первая же перезапись файла приложением
; (`<имя>.tmp` + rename) вернёт унаследованный Users:RX. Постоянное решение —
; перенос секретов в {app}\secure (см. [Dirs]).
Filename: "{sys}\icacls.exe"; \
    Parameters: """{app}\config.json"" /inheritance:r /grant *S-1-5-18:F /grant *S-1-5-32-544:F"; \
    Check: SecretFilePresent('config.json'); \
    StatusMsg: "{cm:StAclCfg}"; Flags: runhidden waituntilterminated
Filename: "{sys}\icacls.exe"; \
    Parameters: """{app}\profiles.json"" /inheritance:r /grant *S-1-5-18:F /grant *S-1-5-32-544:F"; \
    Check: SecretFilePresent('profiles.json'); \
    StatusMsg: "{cm:StAclProf}"; Flags: runhidden waituntilterminated

[UninstallRun]
Filename: "{app}\restlex-agent.exe"; Parameters: "uninstall"; Flags: runhidden; RunOnceId: "UninstAgent"
Filename: "{app}\restlex-webserver.exe"; Parameters: "uninstall"; Flags: runhidden; RunOnceId: "UninstWeb"

[Code]
var
  WebPasswordPage: TInputQueryWizardPage;
  GeneratedApiKey: String;

procedure InitializeWizard;
begin
  WebPasswordPage := CreateInputQueryPage(wpSelectComponents,
    CustomMessage('WebPageTitle'), CustomMessage('WebPageSub'),
    CustomMessage('WebPageText'));
  WebPasswordPage.Add(CustomMessage('WebPwd'), True);
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := (PageID = WebPasswordPage.ID) and not IsComponentSelected('webserver');
end;

function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := True;
  if (CurPageID = WebPasswordPage.ID) and (WebPasswordPage.Values[0] = '') then
  begin
    MsgBox(CustomMessage('WebPwdEmpty'), mbError, MB_OK);
    Result := False;
  end;
end;

function GetApiKey(Param: String): String;
var
  i: Integer;
  Chars: String;
begin
  if GeneratedApiKey = '' then
  begin
    Chars := '0123456789abcdef';
    GeneratedApiKey := '';
    for i := 1 to 32 do
      GeneratedApiKey := GeneratedApiKey + Chars[Random(16) + 1];
  end;
  Result := GeneratedApiKey;
end;

function GetWebPassword(Param: String): String;
begin
  Result := WebPasswordPage.Values[0];
end;

// Секреты в config.json/profiles.json зашифрованы DPAPI LOCAL_MACHINE БЕЗ
// entropy: кто может ПРОЧИТАТЬ файл, тот расшифрует repo_password/tg_token/
// api-ключи парой строк кода. На каталоге у Users осталось чтение (RX, оно
// нужно, чтобы restlex.exe вообще запускался: UAC читает манифест ФИЛЬТРОВАННЫМ
// токеном, где Administrators — deny-only), поэтому файлы приходится закрывать
// точечно. Условие Check для [Run]: файл создаётся агентом при старте службы,
// то есть только к этому моменту он может существовать.
function SecretFilePresent(Name: String): Boolean;
begin
  Result := FileExists(ExpandConstant('{app}\') + Name);
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  KeyFile: String;
  Rc: Integer;
begin
  if (CurStep = ssPostInstall) and IsComponentSelected('webserver') then
  begin
    KeyFile := ExpandConstant('{app}\api-key.txt');
    SaveStringToFile(KeyFile,
      CustomMessage('KeyFileHead') + #13#10 + GeneratedApiKey + #13#10 +
      CustomMessage('KeyFileNote') + #13#10,
      False);
    // Файл с секретом не должен читаться всеми: срезаем наследование и
    // оставляем доступ только SYSTEM и администраторам (SID — имена локализованы).
    Exec(ExpandConstant('{sys}\icacls.exe'),
      '"' + KeyFile + '" /inheritance:r /grant *S-1-5-18:F /grant *S-1-5-32-544:F',
      '', SW_HIDE, ewWaitUntilTerminated, Rc);
  end;
end;
