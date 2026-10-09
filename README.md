# Mongo Compare

A terminal app that checks whether two MongoDB exports contain the same data.

It reads two files produced by `mongoexport` (canonical or relaxed Extended JSON),
pairs up the objects by `_id`, compares every property (including nested objects
and lists), and shows the result in an easy-to-browse report:

- **Do the files match?** One clear answer at the top of the screen.
- **Object counts** for both files, and how many objects are identical, different,
  missing from the destination, or extra in the destination.
- **Mismatches**: the list of object ids that do not match. Open one to see exactly
  which properties differ and what the value is in each file.
- **By date**: the number of records per month, then per day, then the objects of
  that day. You can group by the created date, the updated date, or the time stored
  inside the ObjectId.
- **Fields**: which properties differ most often (for example "`status` differs in 4,210
  objects"). This usually points straight at the cause of a problem.
- **Issues**: lines that could not be read, duplicate `_id`s, documents without `_id`.
- **Export**: save the report as an HTML page (to read or share) and CSV files (to
  open in Excel).

---

## Install

You don't need to install anything else. Mongo Compare is a single program.

### Windows

Open **PowerShell** (press the Windows key, type `PowerShell`, press Enter) and paste:

```powershell
irm https://github.com/SankethJain/CanonicalJsonCompare/releases/latest/download/install.ps1 | iex
```

This creates a **Mongo Compare** shortcut in the Start menu and on the desktop.
No administrator rights are needed.

*Prefer not to run a script?* Download `mongo-compare-x86_64-pc-windows-msvc.zip`
from the [Releases page](https://github.com/SankethJain/CanonicalJsonCompare/releases), then:

1. **Before unzipping**, right-click the zip → **Properties** → tick **Unblock**
   (bottom of the *General* tab) → **OK**.
2. Unzip it and double-click `mongo-compare.exe`.

If Windows still blocks it, see [Windows blocks the program](#windows-blocks-the-program).

#### Windows blocks the program

Windows treats files downloaded with a web browser as "from the internet", and
blocks programs from the internet that are not code-signed. Depending on the
screen you see:

- **"Windows protected your PC"** (blue window, SmartScreen): click **More info**,
  then **Run anyway**. You only need to do this once.
- **No "Run anyway" button**, or the file was removed: unblock the file. Either
  right-click `mongo-compare.exe` → **Properties** → tick **Unblock** → **OK**, or
  open PowerShell in that folder and run:
  ```powershell
  Unblock-File .\mongo-compare.exe
  ```
  Using the PowerShell installer above avoids this entirely: it unblocks the
  program for you.
- **"Smart App Control blocked an app"**, or a message that **your organization**
  blocked it: there is no override on that PC. Smart App Control and company
  policies only allow code-signed programs. Ask your IT team to allow it, or ask
  the maintainers for a signed build (see [Code signing](#code-signing)).

### macOS and Linux

Open **Terminal** and paste:

```sh
curl -fsSL https://github.com/SankethJain/CanonicalJsonCompare/releases/latest/download/install.sh | sh
```

Then open a new Terminal window and type `mongo-compare`.

*Manual install:* download the `.tar.gz` for your computer from the
[Releases page](https://github.com/SankethJain/CanonicalJsonCompare/releases)
(`aarch64-apple-darwin` for Apple Silicon Macs, `x86_64-apple-darwin` for Intel Macs,
`x86_64-unknown-linux-musl` for most Linux PCs), unpack it and run `./mongo-compare`.
On macOS, if you downloaded it with a browser and it is blocked, run
`xattr -d com.apple.quarantine mongo-compare` once.

### For developers

With [Rust](https://rustup.rs) 1.88 or newer:

```sh
cargo install --git https://github.com/SankethJain/CanonicalJsonCompare
```

> **Note:** the one-line installers download from the GitHub Releases of this
> repository. If the repository is private, people without access to it cannot
> download the files; in that case share the files from the Releases page some
> other way (a shared drive, for example).

---

## How to use it

1. **Start it**: open *Mongo Compare* from the Start menu, or type `mongo-compare`
   in a terminal.
2. **Pick the source file** (the original / expected data): press **Enter** on the
   first box to open the file picker. You can also type the path, paste it, or drag
   the file from your file manager into the window.
3. **Pick the destination file** (the copy you want to check) the same way.
4. Press **Enter** on **Start comparison** (or press **F5** anywhere).
5. Explore the results. The keys you can use are always shown at the bottom of the
   screen, and **F1** or **?** shows the full help.

Want to try it first? Press **Ctrl+D** on the first screen (or run
`mongo-compare --demo`) to create two sample files with typical differences.

### Moving around

| Key | What it does |
| --- | --- |
| ↑ ↓, PgUp, PgDn | Move through lists (the mouse wheel and clicks work too) |
| Enter or → | Open the selected row / go one level deeper |
| Esc or ← | Go back one level |
| Tab or 1–5 | Switch between Summary, Mismatches, By date, Fields, Issues |
| f | In a list of objects: show only Different / Only in source / ... |
| / | Search for an object id |
| c | Copy the selected object id |
| a | In an object: switch between "only differences" and "all fields side by side" |
| ← → | In an object: previous / next object |
| d / m | By date: change the date used / show only periods with problems |
| e | Save the report (HTML + CSV) |
| n | New comparison |
| q | Quit |

### What the results mean

| Result | Meaning |
| --- | --- |
| **Identical** | The object exists in both files with exactly the same content. |
| **Different** | Same `_id` in both files, but at least one property differs. |
| **Only in source** | The object is missing from the destination. |
| **Only in destination** | The destination has an object that is not in the source. |

For a *Different* object, every difference is listed with the property path
(for example `customer.address.city` or `items[2].price`) and one of:

- **Value changed**: the value is different.
- **Type changed**: the same value stored with another type, for example the number
  `4` became the text `"4"`, or an Int32 became an Int64.
- **Missing in destination** / **Extra in destination**: the property exists in only
  one of the files.

### Options

The defaults work for most exports, but you can change them on the first screen:

- **Created / Updated date field**: by default the app finds fields such as
  `createdAt`, `created_at`, `updatedAt`, `lastModified`. Type a name (dots work
  for nested fields, e.g. `meta.createdAt`) if yours is called differently.
- **Ignore these fields**: fields that are expected to differ, e.g. `__v, lastSyncedAt`.
  A plain name is ignored at any depth; a dotted path (`meta.syncedAt`) only there.
- **Numbers with the same value are equal**: treat `5`, `5.0`, Int32 and Int64 as the
  same when the value is the same.
- **Ignore the order of items inside lists**: `[1, 2, 3]` equals `[3, 1, 2]`.

The app remembers your last files and options.

### Saving the report

Press **e** on the results screen. A folder named `compare-report-<date>` is created
next to the source file with:

| File | Contents |
| --- | --- |
| `report.html` | The summary, counts per month, top differing fields, and every mismatched object. Opens in any web browser; good for sharing. |
| `differences.csv` | One row per difference: object id, field, source value, destination value. Opens in Excel. |
| `objects.csv` | Every object with its result and dates. |
| `records-by-day.csv` | Counts per day for the created and updated dates. |

---

## Making the export files

Use `mongoexport` for each side, for example:

```sh
mongoexport --uri="mongodb://source-host/mydb" --collection=orders --jsonFormat=canonical --out=orders-source.json
mongoexport --uri="mongodb://dest-host/mydb"   --collection=orders --jsonFormat=canonical --out=orders-dest.json
```

Supported input:

- One document per line (the `mongoexport` default), a JSON array (`--jsonArray`),
  or pretty-printed documents (`--pretty`).
- Canonical (`{"$numberInt": "5"}`, `{"$date": {"$numberLong": "..."}}`) and relaxed
  (`5`, `{"$date": "2024-03-15T10:00:00Z"}`) Extended JSON, even mixed: a date written
  in canonical form in one file and relaxed form in the other is the same date.
- The order of documents does not matter, and neither does the order of fields
  inside a document.
- Files can be large: two files of 300 MB with 1 million objects each compare in
  about 15 seconds using under 1 GB of memory on a 4-core machine.

Dates are grouped by day in UTC.

---

## Automation

The same comparison runs without the interactive screen, for scripts or scheduled jobs:

```sh
mongo-compare --check source.json destination.json --export ./reports
```

It prints a summary and exits with **0** if the files match, **1** if they differ, and
**2** on errors. All the options are available as flags; see `mongo-compare --help`.

Giving two files without `--check` (`mongo-compare a.json b.json`) opens the app and
starts comparing them straight away. On Windows, you can also drag two files onto
`mongo-compare.exe`.

---

## For maintainers

```sh
cargo run                       # start the app
cargo run -- --demo             # start with sample data
cargo test                      # unit tests, including a render test of every screen
cargo clippy --all-targets
```

Code layout (`src/`):

| File | Purpose |
| --- | --- |
| `loader.rs` | Reads export files (lines, arrays, pretty JSON) as a stream |
| `extjson.rs` | Understands Extended JSON types (ObjectId, dates, numbers, ...) |
| `diff.rs` | Compares two documents property by property |
| `engine.rs` | Runs a comparison: pairs documents by `_id`, in parallel batches |
| `report.rs` | The result model, per-field statistics and date grouping |
| `export.rs` | HTML and CSV report |
| `tui/` | The interface (Ratatui): screens, file picker, navigation |

### Code signing

Unsigned Windows programs trigger SmartScreen warnings and are blocked outright
by Smart App Control and many company policies. The release workflow can sign
`mongo-compare.exe` with
[Azure Artifact Signing](https://learn.microsoft.com/azure/artifact-signing/)
(formerly Trusted Signing), a low-cost Microsoft service. Signing is off until
you set it up:

1. In Azure, create an Artifact Signing account, complete identity validation
   (individual or organization) and create a *Public Trust* certificate profile.
2. Create an app registration with a federated credential for this GitHub
   repository (OIDC; no password to store) and give it the
   **Artifact Signing Certificate Profile Signer** role on the account.
3. In GitHub → *Settings* → *Secrets and variables* → *Actions*, add
   - secrets `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID`;
   - variables `AZURE_SIGNING_ENDPOINT` (for example `https://eus.codesigning.azure.net/`),
     `AZURE_SIGNING_ACCOUNT`, `AZURE_CERTIFICATE_PROFILE`, and `WINDOWS_SIGNING` = `true`.
4. Publish a new release (below). The Windows programs are signed before packaging.

A signed program shows the publisher name instead of "Unknown publisher".
SmartScreen may still warn for the first downloads of a new certificate, until
it builds a reputation.

**Releasing a new version:** push a tag named after the version:

```sh
git tag v0.1.2
git push origin v0.1.2
```

The tag sets the version shown in the app and by `mongo-compare --version`, so
there is nothing else to edit. Please also keep `version` in `Cargo.toml` in step,
so local builds show the same number.

The *Release* workflow builds Windows (x64, ARM), macOS (Intel, Apple Silicon) and
Linux (x64, ARM) programs and publishes them, together with the install scripts,
on the Releases page. Windows builds include the C runtime, so nothing else needs
to be installed on users' PCs.
