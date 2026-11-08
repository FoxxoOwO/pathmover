# PathMover

**PathMover** (dříve *LinkVault* / *AppRelocator*) je nativní desktopová aplikace pro Windows 10/11, která uvolňuje
místo na systémovém disku `C:` dvěma způsoby:

1. **Přesouvá těžké složky z `AppData`** na jiný disk (D:, E:, …) a na jejich původním místě zanechá
   **NTFS directory junction**, takže aplikace nic nepozná a funguje dál.
2. **Čistí mezipaměť Electron aplikací** (`Code Cache`, `GPUCache`, `DawnCache`, `Cache`, `logs`, …).

Napsáno v Rustu (edition 2021) s GUI knihovnou `egui`/`eframe` — žádný Electron, žádná webová vrstva,
jen jedno malé nativní okno s tmavým tématem pro power-usery.

```
    C:\Users\...,\AppData\Local\Slack   ──junction──▶   D:\AppDataMoved\Local\Slack
                    (původní cesta)                          (skutečná data)
```

---

## Obsah

- [Funkce](#funkce)
- [Bezpečnost přesunu](#bezpečnost-přesunu)
- [Požadavky](#požadavky)
- [Sestavení a spuštění](#sestavení-a-spuštění)
- [Struktura projektu](#struktura-projektu)
- [Konfigurace](#konfigurace)
- [Jak to funguje technicky](#jak-to-funguje-technicky)
- [Omezení a rizika](#omezení-a-rizika)
- [Řešení potíží](#řešení-potíží)
- [Stav projektu](#stav-projektu)
- [Licence](#licence)

---

## Funkce

Aplikace má tři záložky.

### 1. Přehled disku (Dashboard)

- Volné a celkové místo na `C:` a na všech ostatních pevných discích, včetně pruhu obsazenosti
  (aktualizace každých 10 s přes `sysinfo`).
- Souhrnné dlaždice: volné místo na C:, kolik dat už bylo přesunuto, kolik místa zabírá nalezená cache.
- Varovný banner, pokud aplikace neběží jako správce, s tlačítkem **Restartovat jako správce**
  (elevace přes `ShellExecuteW` + `runas`).
- Nastavení cílového adresáře (`D:\AppDataMoved`), včetně tlačítka *Doporučit největší disk*.
- Rychlé akce: globální sken `AppData\Local` + `AppData\Roaming`, hledání cache.

### 2. AppData & Symlinky (Relocation Engine)

- Prohledá `%LOCALAPPDATA%`, `%APPDATA%` a **kořen profilu** `%USERPROFILE%`, změří velikost
  každé složky prvního řádu (paralelně přes `rayon` + `walkdir`) a seřadí je
  **od největší po nejmenší**.
- Filtry: hledání podle názvu, minimální velikost (logaritmický slider v MB),
  scope Local / Roaming / Profil, „jen přesunuté“, řazení podle velikosti / názvu / stavu.
- Tři scopes: `Local`, `Roaming` a `Profile` (složky přímo v kořeni profilu jako
  `.gradle`, `.cargo`, `.vscode`, `.android`). Přesouvají se do `<cíl>/Local`,
  `<cíl>/Roaming` a `<cíl>/Profile`.
- **Bloklist profilu:** složky, jejichž přesun rozbíjí profil, Known Folder redirection nebo
  cloudový sync, přesunout nelze — zobrazí se jako `Vyloučeno` s důvodem: `AppData`, `Desktop`,
  `Documents`, `Downloads`, `Pictures`, `Music`, `Videos`, `Contacts`, `Links`, `Favorites`,
  `Saved Games`, `Searches`, kompatibilitní junctiony (`Application Data`, `Local Settings`,
  `Cookies`, …), `NTUSER.DAT*`, `OneDrive*`, `.ssh` a `.gnupg`.
  Kořen profilu samotný přesunout nelze nikdy.
- **Indikátor stavu** u každé složky:
  - `Na disku C:` – data jsou fyzicky na systémovém disku,
  - `Přesunuto (D:)` – junction je aktivní a míří na očekávaný cíl,
  - `Odkaz nefunkční` / `Poškozeno` – reparse point existuje, ale cíl chybí nebo je jiný,
  - `Složka nenalezena` – záznam v konfiguraci bez dat na disku.
  - `Vyloučeno` – složka profilu na bloklistu (přesun není povolen).
- **Přesun** jedním tlačítkem s potvrzovacím dialogem, který ukáže zdroj, cíl, velikost i volné místo.
- **Rollback** – tlačítko *Vrátit na C:…* zkopíruje data zpět, odstraní junction a obnoví původní složku.
  Volitelně lze smazat i data na cílovém disku.
- **Historie přesunů** – seznam všech záznamů z `config.toml` s ověřením, zda odkaz stále existuje.

### 3. Čistič cache (Cache Cleaner)

- Najde typické cache složky Electron/Chromium aplikací do hloubky 4 úrovní:
  `Cache`, `Code Cache`, `GPUCache`, `DawnCache`, `DawnGraphiteCache`, `DawnWebGPUCache`,
  `GrShaderCache`, `ShaderCache`, `CachedData`, `CacheStorage`, `Crashpad`, `blob_storage`
  a volitelně `logs`.
- Zobrazí celkovou velikost odpadu, počet souborů a seznam podle aplikací.
- **Vyčistit cache** smaže obsah těchto složek (složky samotné zůstávají).
  Soubory zamčené běžící aplikací se přeskočí a zobrazí se v přehledu chyb — aplikace nikdy nespadne
  kvůli `Access denied` (ekvivalent `try/catch` v PowerShellu).
- Junction odkazy se z čištění vynechávají, aby se omylem nemazala data na jiném disku.
- Po dokončení zobrazí report (uvolněno / souborů / složek / chyb) a automaticky cache přeskenuje.

---

## Bezpečnost přesunu

Přesun není `move` — je to transakce s ověřením v každém kroku a rollbackem při jakékoli chybě.

1. **Preflight**
   - zdroj musí ležet pod `AppData` (jinak odmítnuto),
   - zdroj a cíl musí být na **různých svazcích** (jinak by přesun nic neuvolnil),
   - cílový adresář nesmí existovat,
   - na cílovém disku musí být dost místa: velikost složky + rezerva **256 MiB**.
2. **Kopírování** – rekurzivní kopie s progresem (aktualizace po 64 MiB nebo 120 ms).
   Vnořené reparse pointy se přeskakují (žádné zacyklení). Chyba „disk plný“ (WinError 39/112)
   celou operaci okamžitě ukončí a cíl se uklidí.
3. **Ověření kopie** – porovnání počtu souborů a celkové velikosti zdroje a cíle.
4. **Přejmenování** původní složky na `<název>.bak` (s časovým suffixem, pokud už existuje).
5. **Vytvoření junction** přes `DeviceIoControl(FSCTL_SET_REPARSE_POINT)` s tagem
   `IO_REPARSE_TAG_MOUNT_POINT`; pokud selže, použije se fallback `cmd /C mklink /J`.
6. **Ověření odkazu** – přečtení cíle přes `FSCTL_GET_REPARSE_POINT`, porovnání cest
   a zápis/čtení testovacího souboru `.pathmover_probe` skrz odkaz.
7. **Úklid** – smazání `.bak` (nebo jeho ponechání, pokud je zapnuté *Ponechat .bak zálohu*).
8. **Zápis do `config.toml`** – aplikace si pamatuje, co kam přesunula.

Kdykoli mezi kroky 4–7 dojde k chybě, běží **rollback**: smazání neúplné kopie na cíli, návrat
`.bak` na původní jméno, odstranění případného junction. Uživatel dostane chybovou zprávu a systém
zůstane v konzistentním stavu.

**Rollback (Vrátit na C:)** probíhá stejně obezřetně: kopie do dočasné složky
`.pathmover-restore-<název>` vedle originálu → ověření → odstranění junction → přejmenování na originál.
Když se cokoli pokazí, junction se znovu vytvoří a data zůstanou dostupná.

---

## Požadavky

| Co | Verze / poznámka |
|---|---|
| OS | Windows 10 nebo Windows 11 (junction = funkce NTFS) |
| Rust | 1.75+ (edition 2021) |
| Cílový disk | **NTFS** (FAT32/exFAT junction nepodporuje) |
| Oprávnění | Administrátor pro vytváření junction a zápis do některých složek AppData |
| Grafika | OpenGL (backend `glow`, součást `eframe`) |

### Toolchain (důležité pro Windows-GNU)

Projekt je vyvíjen na hostitelském toolchainu `x86_64-pc-windows-gnu`. Crate `windows-sys` používá
`raw-dylib`, takže linker potřebuje `dlltool` **a** assembler `as`:

- **MSVC toolchain** (`stable-x86_64-pc-windows-msvc`) — stačí Visual Studio C++ Build Tools
  (link.exe, Windows SDK). Doporučeno, pokud je máte.
- **GNU toolchain** — kromě `rustup` je potřeba MinGW-w64 (WinLibs). Bez něj build skončí chybou
  `error calling dlltool 'dlltool.exe': program not found` nebo
  `dlltool could not create import library`.

```powershell
# MinGW-w64 (WinLibs) do PATH - dlltool/as pro raw-dylib crate windows-sys
$env:PATH = "C:\tools\mingw64\bin;$env:PATH"
cargo build
```

Pro trvalé nastavení stačí přidat `C:\tools\mingw64\bin` do systémové `PATH`. Nástroje jsou potřeba
jen při kompilaci, ne při běhu aplikace.

---

## Sestavení a spuštění

```powershell
git clone <repo> pathmover
cd pathmover

# vývojový build
cargo run

# release build (optimalizováno, stripped)
cargo build --release
.\target\release\pathmover.exe
```

Testy:

```powershell
cargo test
```

### Spuštění jako správce

Doporučená cesta je z terminálu spuštěného jako administrátor:

```powershell
Start-Process -Verb RunAs .\target\release\pathmover.exe
```

Nebo z UI: pokud aplikace neběží elevovaně, zobrazí se banner s tlačítkem
**Restartovat jako správce** (spustí novou instanci a stávající okno zavřete).

Trvalou elevaci lze vynutit přidáním manifestu `pathmover.exe.manifest` s
`requestedExecutionLevel level="requireAdministrator"` (viz [Stav projektu](#stav-projektu)).

---

## Struktura projektu

| Soubor | Řádků | Obsah |
|---|---:|---|
| `src/main.rs` | 35 | vstupní bod, `eframe::run_native`, okno 1120×760, ikona, Windows-only guard |
| `src/app.rs` | 865 | stav aplikace, záložky, fronty zpráv z vláken, `eframe::App` |
| `src/util.rs` | 198 | formátování velikostí/času, normalizace cest, `paths_eq`, testy |
| `src/winapi.rs` | 425 | junction (FSCTL get/set reparse point), `mklink /J` fallback, volné místo, elevace, Průzkumník |
| `src/fsutil.rs` | 373 | rekurzivní kopie s progresem, ověření kopie, mazání stromu, probe soubor, testy |
| `src/scanner.rs` | 333 | sken AppData (rayon), klasifikace stavu složky, `FolderEntry`, testy |
| `src/relocator.rs` | 550 | přesun/rollback jako transakce s fázemi a eventy, testy |
| `src/cleaner.rs` | 265 | hledání cache složek, bezpečné mazání, `CleanReport`, testy |
| `src/config.rs` | 213 | `config.toml` (serde), seznam přesunů, detekce pevných disků, testy |
| `src/ui/mod.rs` | 446 | kořenové UI: horní lišta, záložky, stavový řádek, log panel, potvrzovací dialogy |
| `src/ui/theme.rs` | 193 | tmavá paleta, fonty Segoe UI/Consolas, procedurální ikona |
| `src/ui/widgets.rs` | 187 | karty, chipy, stat dlaždice, progress bary, tlačítka |
| `src/ui/dashboard.rs` | 338 | záložka Přehled disku |
| `src/ui/relocate.rs` | 398 | záložka AppData & Symlinky, historie přesunů |
| `src/ui/cleaner.rs` | 232 | záložka Čistič cache, report posledního čištění |

Architektura: UI běží v hlavním vlákně, každá dlouhá operace (sken, kopírování, mazání) v samostatném
`std::thread` a komunikuje přes `mpsc` kanál zprávami (`app::Msg`). UI se překresluje na vyžádání
(`Context::request_repaint`) a každá operace má `AtomicBool` pro zrušení.

---

## Konfigurace

Ukládá se do `%APPDATA%\PathMover\config.toml`:

```toml
version = 1
target_root = "D:\\AppDataMoved"
keep_backup = false      # ponechat .bak zálohu po přesunu
clean_logs = false       # zahrnout logs/crash dumps do hledání cache

[[entries]]
scope = "Local"          # Local | Roaming | Profile
name = "Slack"
source = "C:\\Users\\Honza\\AppData\\Local\\Slack"
target = "D:\\AppDataMoved\\Local\\Slack"
bytes = 3221225472
moved_at = 1717171717    # unix timestamp
```

Soubor se vytvoří při prvním uložení (změna cíle, přesun, rollback). Při startu aplikace ověří,
že každý záznam má platný junction, a rozbité záznamy zobrazí jako varování.

---

## Jak to funguje technicky

- **Junction bez admin promptu na každý soubor:** reparse point se zapisuje přímo přes
  `CreateFileW` + `DeviceIoControl`. Cílová cesta musí být v „print name“ formě (`\??\D:\...`
  interně, `D:\...` čitelně), proto se cesty před zápisem převádějí na extended formu `\\?\`.
- **Skip reparse pointů při skenování:** `walkdir` s `follow_links(false)` + filtr na
  `FILE_ATTRIBUTE_REPARSE_POINT` (0x400), aby se junction neprocházely dokola a velikosti se
  nepočítaly dvakrát.
- **Volné místo** přes `GetDiskFreeSpaceExW` (`available_space`), ne z `total - free`.
- **Mazání** je „contents first“: nejdřív soubory, pak složky; u zamčených souborů se zkusí
  zrušit atributy `READONLY/HIDDEN/SYSTEM` přes `SetFileAttributesW` a smazání se zopakuje.
- **Vlákna a progres:** `copy_tree` hlásí každých 64 MiB nebo 120 ms, takže progress bar není
  ani zaseknutý, ani nezahlcuje UI.

---

## Omezení a rizika

- **Zavřete aplikaci, kterou přesouváte.** Běžící proces drží soubory otevřené → kopie může být
  neúplná nebo přejmenování na `.bak` selže.
- **Junction nejde na FAT32/exFAT/SD kartu** ani na síťový disk. Cílem musí být lokální NTFS.
- **OneDrive / Dropbox / Google Drive** složky: přesun může rozbít synchronizaci. Nedoporučeno.
- **Nepřesouvejte** `AppData\Local\Packages`, `Microsoft`, `Temp` a složky, které používají
  služby běžící pod SYSTEM — junction pod nimi může selhat kvůli oprávněním.
- **Složky profilu** vyloučené bloklistem nejde přesunout vůbec (viz výše); ostatní složky
  profilu přesunout lze, ale platí u nich dvojnásob „zavřete aplikaci“ — řada nástrojů
  (`git`, `cargo`, `gradle`) si drží soubory otevřené a přesun za běhu selže.
- **Rollback potřebuje volné místo na C:** (velikost složky + rezerva).
- **Antivirus** může blokovat zápis reparse pointu; v takovém případě pomůže výjimka.
- Aplikace záměrně **nemaže** data bez potvrzení a nikdy nemaže obsah, který leží mimo `AppData`.

> ⚠️ Přesun složek v AppData je zásah do systému. Před prvním použitím si udělejte bod obnovení
> (`Checkpoint-Computer -Description "Před PathMover"`) nebo zálohu.

---

## Řešení potíží

| Příznak | Příčina | Řešení |
|---|---|---|
| `Přístup byl odepřen` při vytváření odkazu | aplikace neběží jako správce | spusťte jako administrátor nebo použijte banner v Dashboardu |
| `Cílový adresář již existuje` | na cíli už jsou data ze staršího pokusu | smažte/ přejmenujte cíl, nebo použijte *Vrátit na C:* a zkuste znovu |
| `na cílovém disku došlo místo` | málo místa (rezerva 256 MiB) | uvolněte místo, nebo zvolte jiný disk |
| Složka po přesunu „zmizela“ | junction se nevytvořil, zůstala `.bak` | najděte `<název>.bak` v AppData a přejmenujte zpět |
| `Složka je zamčená` při kopírování | aplikace běží | zavřete aplikaci (i tray ikonu) a zkuste znovu |
| Cache se nevyčistila celá | soubory drží běžící prohlížeč/Electron | zavřete aplikaci, obnovte sken a čistěte znovu |
| V seznamu je `Odkaz nefunkční` | cíl byl smazán nebo odpojen disk | připojte disk, nebo složku vraťte na C: |
| UI zamrzlé | čeká na dokončení operace | použijte *Zrušit*, operace se ukončí po aktuálním souboru |
| `error calling dlltool 'dlltool.exe': program not found` | Windows-GNU toolchain bez MinGW-w64 | přidejte `C:\tools\mingw64\bin` (WinLibs) do `PATH`, viz *Toolchain* |
| `dlltool could not create import library` | chybí assembler `as` pro `raw-dylib` | totéž — dlltool volá `as`, samotný rustup ho nedodá |
| `compile_error!("PathMover is a Windows-only application…")` | build na Linuxu/macOS | projekt cílí na Windows 10/11 |

Logy a chybové zprávy jsou v panelu **Log** ve spodní části okna (lze rozbalit).

---

## Stav projektu

Aplikace je **kompletní a zkompilovatelná**: `cargo build` prochází bez varování a `cargo test`
hází 18/18 zelených testů (`util`, `winapi`, `fsutil`, `scanner`, `relocator`, `cleaner`, `config`),
včetně roundtrip testu skutečného NTFS junction a testů kopírování/mazání stromu.

- `src/main.rs` — `eframe::run_native` s `ViewportBuilder` (ikona, min. rozměr, tmavé téma)
- `src/ui/mod.rs` — horní lišta se záložkami, stavový řádek s progresem, log panel,
  potvrzovací modální dialogy (přesun i rollback) včetně kontroly volného místa
- `src/ui/cleaner.rs` — záložka Čistič cache (výběr, mazání, report)

Spuštění je ověřeno smoke testem (proces běží, GUI se otevře). Plánované doplňky: manifest pro
vynucenou elevaci, virtualizovaný seznam složek (pro tisíce položek), export reportu do CSV.

---

## Licence

MIT (nebo dle volby autora). Projekt vzniká jako nástroj pro vlastní potřebu — používejte na vlastní
riziko a se zálohou.