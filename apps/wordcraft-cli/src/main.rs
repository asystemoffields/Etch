//! `etch-cli`: Etch from the command line.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod script;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};
use wordcraft_engine::Session;

const USAGE: &str = "\
etch-cli — Etch from the command line

USAGE:
  etch-cli convert <in> <out> [--markup] [--update-fields] [--format wordml2003|flatopc]
                                              convert between formats (docx, doc [read], pdf, odt, rtf, html, md, tex, txt, json, png,
                                              xml); --update-fields refreshes unlocked fields before saving;
                                              an .xml file is a Word XML Document (Flat OPC), or with
                                              --format wordml2003 a Word 2003 XML Document;
                                              --markup (Print Markup): comments beside each page and
                                              tracked changes in pdf/png, a comment list in html/md/txt
  etch-cli info <file>                        pages, words, paragraphs, properties (JSON)
  etch-cli text <file>                        plain text
  etch-cli inspect <file>                     document structure (JSON)
  etch-cli render <file> <out.png> [--page N] [--scale S]
  etch-cli run [--file F | --template T] [--script FILE|-] --cmd 'id={json}' [--cmd …]
               [--save OUT] [--print] [--author NAME]
                                              run commands headlessly, then save (--author names
                                              who tracked changes and comments are by).
                                              --script runs a JSON file (- is stdin) first:
                                              {\"file\"?, \"template\"?, \"commands\": [{\"command\": id,
                                              \"params\"?: {…}} | id, …], \"save\"?: [OUT, …]}; save
                                              formats follow the extension (docx and pdf in one run);
                                              relative paths resolve against the script's folder
  etch-cli commands [--json]                  list every command
  etch-cli parity [--markdown]                feature-catalog parity
  etch-cli zotero <command> <file> [--cmd 'id={json}' …] [--save OUT] [--trace] [--port P]
                                              run a Zotero command on a document (Zotero must be
                                              running): addEditCitation, addEditBibliography,
                                              addNote, refresh, removeCodes, setDocPrefs
                                              (--cmd runs first, e.g. --cmd caret.docEnd)
  etch-cli [<command>] -h | --help            this help
  etch-cli --version

A password-protected Word file is opened with --password PASS, --password-file PATH or the
WORDCRAFT_PASSWORD environment variable; a password given on the command line is readable by
every other process, so the other two are better. A file opened with a password is written with
it again, as the app does. Exports that cannot carry a password (pdf, html, txt) stay plain.

Paper: documents created here, and HTML, Markdown and text files (which give no page size), use
the system region's paper (A4, or Letter in the US and other Letter regions), read from
LC_ALL / LC_PAPER / LANG or the platform. Set WORDCRAFT_REGION=en-US (or another tag) for the
same output on every machine.
";

/// The subcommands, in `USAGE` order; `--help` or `-h` after any of them prints `USAGE` (#968).
const SUBCOMMANDS: &[&str] = &["convert", "info", "text", "inspect", "render", "run", "commands", "parity", "zotero"];

fn wants_help(cmd: &str, rest: &[String]) -> bool {
    SUBCOMMANDS.contains(&cmd) && rest.iter().any(|a| a == "--help" || a == "-h")
}

/// The error every subcommand gives when a required path is not there.
fn missing_argument() -> Failure {
    format!("missing argument\n\n{USAGE}").into()
}

/// Open a document for a subcommand. A password-protected Word file needs its password
/// ([`password_from`]). As in the app, a document opened with one is saved with it again, so
/// `convert` does not quietly write a copy the password no longer protects.
fn open(path: &Path, password: Option<&str>) -> Result<Session, String> {
    // Through `file.open`'s own code, so the session keeps the file's identity and a save refuses
    // to overwrite a version someone else changed (#813), for any file name (#1440). A file that
    // gives no page size (HTML, Markdown, plain text) opens on the system region's paper, as in
    // the app: A4 in Italy, Letter in the US (#1381).
    let mut s = session(wordcraft_doc::Document::new());
    wordcraft_engine::cmd::file::open_path(&mut s, path, password).map_err(|e| e.to_string())?;
    Ok(s)
}

/// The commands that open a file, their options, and which of their positional arguments
/// ([`positionals`]) the file is. `run` opens the value of `--file`.
const OPENS_FILE: &[(&str, &[OptionSpec], usize)] = &[
    ("convert", CONVERT_OPTIONS, 0),
    ("info", PASSWORD_OPTIONS, 0),
    ("text", PASSWORD_OPTIONS, 0),
    ("inspect", PASSWORD_OPTIONS, 0),
    ("render", RENDER_OPTIONS, 0),
    ("zotero", ZOTERO_OPTIONS, 1),
];

/// The arguments as text. Only the file the command opens may have a name that isn't valid Unicode,
/// such as a Latin-1 one (#1440): it comes back as the path to open, and lossy among the arguments.
/// Any other such argument is an error naming it.
fn text_args<S: AsRef<OsStr>>(raw: &[S]) -> Result<(Vec<String>, Option<PathBuf>), String> {
    let args: Vec<String> = raw.iter().map(|a| a.as_ref().to_string_lossy().into_owned()).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let rest = args.get(1..).unwrap_or_default();
    // Indices into `raw`, which has the command before `rest`.
    let file_at = match OPENS_FILE.iter().find(|(c, _, _)| *c == cmd) {
        Some((_, specs, n)) => positional_indices(rest, specs).get(*n).map(|i| i + 1),
        None if cmd == "run" => rest.iter().position(|a| a == "--file").map(|i| i + 2),
        None => None,
    };
    // `run --script FILE` may also name its script with such a file name.
    let script_at = if cmd == "run" { args.iter().position(|a| a == "--script").map(|i| i + 1) } else { None };
    let mut file = None;
    for (i, a) in raw.iter().map(AsRef::as_ref).enumerate() {
        if a.to_str().is_none() {
            if Some(i) != file_at && Some(i) != script_at {
                return Err(format!("argument {} is not valid Unicode: {}", i + 1, a.to_string_lossy()));
            }
            if Some(i) == file_at {
                file = Some(PathBuf::from(a));
            }
        }
    }
    Ok((args, file))
}

/// The password for a protected Word file: `--password`, `--password-file`, or
/// `WORDCRAFT_PASSWORD`, in that order. The file form exists because a password on a command line
/// is readable by every other process on the machine; a trailing newline is dropped so a file
/// written by `echo` works. Never logged, and never named in an error.
fn password_from(args: &[String]) -> Result<Option<String>, String> {
    if let Some(password) = arg_value(args, "--password") {
        return Ok(Some(password));
    }
    if let Some(path) = arg_value(args, "--password-file") {
        let raw = std::fs::read_to_string(&path).map_err(|e| format!("--password-file {path}: {e}"))?;
        return Ok(Some(raw.trim_end_matches(['\r', '\n']).to_string()));
    }
    Ok(std::env::var("WORDCRAFT_PASSWORD").ok().filter(|p| !p.is_empty()))
}

/// A headless session in the system region: every way the CLI starts one gives `file.new` the
/// same paper (#1381).
fn session(doc: wordcraft_doc::Document) -> Session {
    let mut s = Session::new(doc);
    s.system_region = system_region();
    s
}

/// The system region, read as the app reads it (locale variables, else the platform's locale).
fn system_region() -> wordcraft_engine::Region {
    wordcraft_engine::Region::from_locale_vars(|name| std::env::var(name).ok(), sys_locale::get_locale)
}

/// The body text for `text`: the plain-text export, so list items keep their bullet or number label.
fn listed_text(doc: &wordcraft_doc::Document) -> Result<String, String> {
    let bytes = wordcraft_engine::io::save_bytes("out.txt", doc)?;
    Ok(String::from_utf8_lossy(&bytes).replace("\r\n", "\n"))
}

/// What stderr says after a save to `out` returned `result`: the file written and, when its
/// format can't hold all of the document, what the file is without (`file.save`'s `lossy`:
/// comments, footnotes…), so a conversion never drops content without a word. (The app asks
/// before such a save; a script is told.)
fn wrote(out: &str, result: &Value) -> String {
    let lost: Vec<&str> = result.get("lossy").and_then(Value::as_array).map(|l| l.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    // A PDF is a picture of the pages (headers, notes and comments with --markup are on them),
    // not a document to edit again: nothing to warn about.
    let picture = out.to_ascii_lowercase().ends_with(".pdf");
    if lost.is_empty() || picture {
        format!("wrote {out}")
    } else {
        format!("wrote {out}\nwarning: {out} is without the document's {}: its format can't keep them", lost.join(", "))
    }
}

/// Save the session to `out`: `file.save`, or `file.exportPng` with `png` for a `.png`; what
/// [`wrote`] says about it.
fn save_to(s: &mut Session, out: &str, png: Value) -> Result<String, String> {
    let r = if out.to_ascii_lowercase().ends_with(".png") { s.run("file.exportPng", &png) } else { s.run("file.save", &json!({"path": out})) };
    Ok(wrote(out, &r.map_err(|e| e.to_string())?))
}

/// `--author NAME`: the author recorded on tracked changes and comments (`file.setAuthor`).
fn set_author(s: &mut Session, name: &str) -> Result<(), String> {
    s.run("file.setAuthor", &json!({"name": name})).map(|_| ()).map_err(|e| format!("--author: {e}"))
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

enum OptionArg<'a> {
    Absent,
    Duplicate,
    MissingValue,
    Value(&'a str),
}

fn find_option_arg<'a>(args: &'a [String], name: &str) -> OptionArg<'a> {
    let mut matches = args.iter().enumerate().filter(|(_, a)| *a == name);
    let Some((i, _)) = matches.next() else {
        return OptionArg::Absent;
    };
    if matches.next().is_some() {
        return OptionArg::Duplicate;
    }
    match args.get(i + 1) {
        None => OptionArg::MissingValue,
        Some(val) if val.starts_with("--") => OptionArg::MissingValue,
        Some(val) => OptionArg::Value(val.as_str()),
    }
}

fn parse_page_arg(command: &str, args: &[String]) -> Result<Option<u64>, String> {
    match find_option_arg(args, "--page") {
        OptionArg::Absent => Ok(None),
        OptionArg::Duplicate => Err(format!("{command}: duplicate --page option")),
        OptionArg::MissingValue => Err(format!("{command}: --page requires a value")),
        OptionArg::Value(val) => {
            let n = val.parse::<u64>().map_err(|_| format!("{command}: invalid --page `{val}`: not a valid integer"))?;
            if n == 0 {
                return Err(format!("{command}: invalid --page `0`: page numbers start at 1"));
            }
            Ok(Some(n))
        }
    }
}

fn parse_scale_arg(command: &str, args: &[String]) -> Result<Option<f64>, String> {
    match find_option_arg(args, "--scale") {
        OptionArg::Absent => Ok(None),
        OptionArg::Duplicate => Err(format!("{command}: duplicate --scale option")),
        OptionArg::MissingValue => Err(format!("{command}: --scale requires a value")),
        OptionArg::Value(val) => {
            let scale = val.parse::<f64>().map_err(|_| format!("{command}: invalid --scale `{val}`: not a number"))?;
            if !scale.is_finite() || scale <= 0.0 {
                return Err(format!("{command}: invalid --scale `{val}`: scale must be a positive number"));
            }
            Ok(Some(scale))
        }
    }
}

#[derive(Clone, Copy)]
struct OptionSpec {
    name: &'static str,
    takes_value: bool,
}

fn validate_options(command: &str, args: &[String], specs: &[OptionSpec]) -> Result<(), String> {
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if let Some(name) = arg.strip_prefix("--") {
            let Some(spec) = specs.iter().find(|spec| spec.name == arg) else {
                return Err(format!("{command}: unknown option --{name}"));
            };
            if spec.takes_value && args.get(i + 1).is_some_and(|value| !value.starts_with("--")) {
                i += 2;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    Ok(())
}

/// The positional arguments: everything that is not an option or the value of an option that
/// takes one (`render in.docx --page 2 out.png` has `in.docx` and `out.png`). Mirrors
/// [`validate_options`].
fn positionals<'a>(args: &'a [String], specs: &[OptionSpec]) -> Vec<&'a String> {
    positional_indices(args, specs).into_iter().filter_map(|i| args.get(i)).collect()
}

/// Where in `args` the [`positionals`] are.
fn positional_indices(args: &[String], specs: &[OptionSpec]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if arg.starts_with("--") {
            let takes_value = specs.iter().any(|spec| spec.name == arg && spec.takes_value);
            i += if takes_value && args.get(i + 1).is_some_and(|value| !value.starts_with("--")) { 2 } else { 1 };
        } else {
            out.push(i);
            i += 1;
        }
    }
    out
}

/// The password a protected Word file is opened with: `--password` (which any other process on
/// the machine can read from the command line), `--password-file`, or `WORDCRAFT_PASSWORD`.
const PASSWORD_OPTIONS: &[OptionSpec] =
    &[OptionSpec { name: "--password", takes_value: true }, OptionSpec { name: "--password-file", takes_value: true }];
const CONVERT_OPTIONS: &[OptionSpec] = &[
    OptionSpec { name: "--markup", takes_value: false },
    OptionSpec { name: "--format", takes_value: true },
    OptionSpec { name: "--update-fields", takes_value: false },
    OptionSpec { name: "--password", takes_value: true },
    OptionSpec { name: "--password-file", takes_value: true },
];
const RENDER_OPTIONS: &[OptionSpec] = &[
    OptionSpec { name: "--page", takes_value: true },
    OptionSpec { name: "--scale", takes_value: true },
    OptionSpec { name: "--password", takes_value: true },
    OptionSpec { name: "--password-file", takes_value: true },
];
const RUN_OPTIONS: &[OptionSpec] = &[
    OptionSpec { name: "--file", takes_value: true },
    OptionSpec { name: "--password", takes_value: true },
    OptionSpec { name: "--password-file", takes_value: true },
    OptionSpec { name: "--template", takes_value: true },
    OptionSpec { name: "--cmd", takes_value: true },
    OptionSpec { name: "--script", takes_value: true },
    OptionSpec { name: "--save", takes_value: true },
    OptionSpec { name: "--page", takes_value: true },
    OptionSpec { name: "--scale", takes_value: true },
    OptionSpec { name: "--print", takes_value: false },
    OptionSpec { name: "--author", takes_value: true },
];
const COMMANDS_OPTIONS: &[OptionSpec] = &[OptionSpec { name: "--json", takes_value: false }];
const PARITY_OPTIONS: &[OptionSpec] = &[OptionSpec { name: "--markdown", takes_value: false }];
const ZOTERO_OPTIONS: &[OptionSpec] = &[
    OptionSpec { name: "--cmd", takes_value: true },
    OptionSpec { name: "--save", takes_value: true },
    OptionSpec { name: "--trace", takes_value: false },
    OptionSpec { name: "--port", takes_value: true },
];

/// Why a CLI run stopped early.
#[derive(Debug, PartialEq)]
enum Failure {
    /// A real error, reported on stderr with a failing exit status.
    Error(String),
    /// Whoever read stdout closed it (`wordcraft-cli commands --json | head`): there is nobody left
    /// to write to, so stop quietly and exit 0 like other command-line tools.
    StdoutClosed,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Error(e) => f.write_str(e),
            Failure::StdoutClosed => f.write_str("stdout closed"),
        }
    }
}

impl From<String> for Failure {
    fn from(e: String) -> Self {
        Failure::Error(e)
    }
}

impl From<&str> for Failure {
    fn from(e: &str) -> Self {
        Failure::Error(e.into())
    }
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::BrokenPipe { Failure::StdoutClosed } else { Failure::Error(e.to_string()) }
    }
}

/// Write to stdout through one locked handle and flush. Unlike `println!`, a closed stdout comes
/// back as an error (`Failure::StdoutClosed`) instead of a panic.
fn write_stdout(write: impl FnOnce(&mut std::io::StdoutLock<'static>) -> std::io::Result<()>) -> Result<(), Failure> {
    let mut out = std::io::stdout().lock();
    write(&mut out)?;
    out.flush()?;
    Ok(())
}

/// The operand after `--script`, by its real name (it may not be valid Unicode).
fn script_operand<S: AsRef<OsStr>>(raw: &[S]) -> Option<PathBuf> {
    let at = raw.iter().position(|a| a.as_ref() == OsStr::new("--script"))?;
    raw.get(at + 1).map(|a| PathBuf::from(a.as_ref()))
}

fn run<S: AsRef<OsStr>>(args: &[S]) -> Result<(), Failure> {
    let script_path = script_operand(args);
    let (args, file) = text_args(args)?;
    // Opens the file by its real name, which its text in `args` may have lost, and with the
    // password of a protected document (`password_from`).
    let open = |text: &str, password: Option<&str>| open(file.as_deref().unwrap_or(Path::new(text)), password);
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();
    // Before options are checked or files and folders opened.
    if wants_help(cmd, &rest) {
        return write_stdout(|o| write!(o, "{USAGE}"));
    }
    let pos = |specs: &[OptionSpec], i: usize| positionals(&rest, specs).get(i).map(|a| (*a).clone()).ok_or_else(missing_argument);
    match cmd {
        "convert" => {
            validate_options("convert", &rest, CONVERT_OPTIONS)?;
            let (i, o) = (pos(CONVERT_OPTIONS, 0)?, pos(CONVERT_OPTIONS, 1)?);
            // `--format wordml2003` (or `flatopc`) picks the kind of `.xml` file; its value isn't a path.
            let format = arg_value(&rest, "--format");
            let markup = rest.iter().any(|a| a == "--markup");
            let password = password_from(&rest)?;
            let mut s = open(&i, password.as_deref())?;
            if rest.iter().any(|a| a == "--update-fields") {
                s.run("references.updateFields", &json!({})).map_err(|e| e.to_string())?;
            }
            let r = if o.to_ascii_lowercase().ends_with(".png") {
                s.run("file.exportPng", &json!({"path": o, "page": 1, "markup": markup}))
            } else {
                let mut params = json!({"path": o, "markup": markup});
                if let Some(f) = format {
                    params["format"] = json!(f);
                }
                s.run("file.save", &params)
            };
            eprintln!("{}", wrote(&o, &r.map_err(|e| e.to_string())?));
            Ok(())
        }
        "info" => {
            validate_options("info", &rest, PASSWORD_OPTIONS)?;
            let password = password_from(&rest)?;
            let mut s = open(&pos(PASSWORD_OPTIONS, 0)?, password.as_deref())?;
            let info = serde_json::to_string_pretty(&s.run("file.info", &json!({})).map_err(|e| e.to_string())?).unwrap_or_default();
            write_stdout(|o| writeln!(o, "{info}"))
        }
        "text" => {
            validate_options("text", &rest, PASSWORD_OPTIONS)?;
            let password = password_from(&rest)?;
            let s = open(&pos(PASSWORD_OPTIONS, 0)?, password.as_deref())?;
            let text = listed_text(&s.doc)?;
            write_stdout(|o| writeln!(o, "{text}"))
        }
        "inspect" => {
            validate_options("inspect", &rest, PASSWORD_OPTIONS)?;
            let password = password_from(&rest)?;
            let mut s = open(&pos(PASSWORD_OPTIONS, 0)?, password.as_deref())?;
            let tree = serde_json::to_string_pretty(&s.run("document.inspect", &json!({})).map_err(|e| e.to_string())?).unwrap_or_default();
            write_stdout(|o| writeln!(o, "{tree}"))
        }
        "render" => {
            validate_options("render", &rest, RENDER_OPTIONS)?;
            let (i, o) = (pos(RENDER_OPTIONS, 0)?, pos(RENDER_OPTIONS, 1)?);
            let password = password_from(&rest)?;
            let page = parse_page_arg("render", &rest)?.unwrap_or(1);
            let scale = parse_scale_arg("render", &rest)?.unwrap_or(2.0);
            let mut s = open(&i, password.as_deref())?;
            let r = s.run("file.exportPng", &json!({"path": o, "page": page, "scale": scale})).map_err(|e| e.to_string())?;
            write_stdout(|o| writeln!(o, "{r}"))
        }
        "run" => {
            validate_options("run", &rest, RUN_OPTIONS)?;
            let page = parse_page_arg("run", &rest)?;
            let scale = parse_scale_arg("run", &rest)?;
            let password = password_from(&rest)?;
            let script = match arg_value(&rest, "--script") {
                Some(path) if !path.starts_with("--") => Some(script::load(script_path.as_deref().unwrap_or(Path::new(&path)))?),
                _ if rest.iter().any(|a| a == "--script") => return Err("run: --script requires a value".into()),
                _ => None,
            };
            // A source named on the command line wins over both of the script's.
            let (mut from_file, mut from_template) = (arg_value(&rest, "--file"), arg_value(&rest, "--template"));
            if from_file.is_none() && from_template.is_none() {
                from_file = script.as_ref().and_then(|c| c.file.clone());
                from_template = script.as_ref().and_then(|c| c.template.clone());
            }
            let mut s = match (from_file, from_template) {
                (Some(f), _) => open(&f, password.as_deref())?,
                (None, Some(t)) => {
                    let mut s = session(wordcraft_doc::Document::new());
                    s.run("file.new", &json!({"template": t})).map_err(|e| e.to_string())?;
                    s
                }
                _ => session(wordcraft_doc::Document::new()),
            };
            if let Some(name) = arg_value(&rest, "--author") {
                set_author(&mut s, &name)?;
            }
            let print = rest.iter().any(|a| a == "--print");
            for (n, (id, params)) in script.iter().flat_map(|c| c.commands.iter()).enumerate() {
                let r = s.run(id, params).map_err(|e| format!("command {} ({id}): {e}", n + 1))?;
                if print {
                    write_stdout(|o| writeln!(o, "{id}: {r}"))?;
                }
            }
            let mut i = 0;
            while i < rest.len() {
                if rest.get(i).map(String::as_str) == Some("--cmd") {
                    let spec = rest.get(i + 1).ok_or("--cmd needs a value")?;
                    let (id, params) = match spec.split_once('=') {
                        Some((id, p)) => (id.to_string(), serde_json::from_str::<Value>(p).map_err(|e| format!("{id}: bad JSON params: {e}"))?),
                        None => (spec.clone(), json!({})),
                    };
                    let r = s.run(&id, &params).map_err(|e| format!("{id}: {e}"))?;
                    if print {
                        write_stdout(|o| writeln!(o, "{id}: {r}"))?;
                    }
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let outputs: Vec<String> = script.iter().flat_map(|c| c.save.iter().cloned()).chain(arg_value(&rest, "--save")).collect();
            // Check every output can be made before writing any, so a bad one leaves no half-done job.
            for out in &outputs {
                let parent = Path::new(out).parent().filter(|p| !p.as_os_str().is_empty());
                if let Some(p) = parent.filter(|p| !p.is_dir()) {
                    return Err(format!("{out}: folder {} does not exist", p.display()).into());
                }
                if Path::new(out).is_dir() {
                    return Err(format!("{out}: is a folder, not a file").into());
                }
                if !out.to_ascii_lowercase().ends_with(".png") {
                    wordcraft_engine::io::save_bytes(out, &s.doc)?;
                }
            }
            for out in outputs {
                let mut png = json!({"path": out});
                if let Some(n) = page {
                    png["page"] = json!(n);
                }
                if let Some(k) = scale {
                    png["scale"] = json!(k);
                }
                eprintln!("{}", save_to(&mut s, &out, png)?);
            }
            Ok(())
        }
        "commands" => {
            validate_options("commands", &rest, COMMANDS_OPTIONS)?;
            let s = session(wordcraft_doc::Document::new());
            if rest.iter().any(|a| a == "--json") {
                let list = serde_json::to_string_pretty(&s.registry.describe()).unwrap_or_default();
                write_stdout(|o| writeln!(o, "{list}"))
            } else {
                write_stdout(|o| {
                    for c in s.registry.all() {
                        writeln!(o, "{:<32} {:<36} {:<24} {}", c.id, c.label, c.shortcut, c.location)?;
                    }
                    Ok(())
                })
            }
        }
        "parity" => {
            validate_options("parity", &rest, PARITY_OPTIONS)?;
            let s = session(wordcraft_doc::Document::new());
            let p = wordcraft_engine::catalog::parity(&s.registry);
            if rest.iter().any(|a| a == "--markdown") {
                write_stdout(|o| write!(o, "{}", parity_markdown(&p)))
            } else {
                write_stdout(|o| writeln!(o, "{}", serde_json::to_string_pretty(&p).unwrap_or_default()))
            }
        }
        "zotero" => {
            validate_options("zotero", &rest, ZOTERO_OPTIONS)?;
            let name = pos(ZOTERO_OPTIONS, 0)?;
            let command = wordcraft_zotero::Command::from_name(&name).ok_or_else(|| format!("unknown Zotero command `{name}`\n\n{USAGE}"))?;
            let file = pos(ZOTERO_OPTIONS, 1)?;
            let mut s = open(&file, None)?;
            for spec in rest.iter().zip(rest.iter().skip(1)).filter(|(a, _)| *a == "--cmd").map(|(_, v)| v) {
                let (id, params) = match spec.split_once('=') {
                    Some((id, p)) => (id.to_string(), serde_json::from_str::<Value>(p).map_err(|e| format!("{id}: bad JSON params: {e}"))?),
                    None => (spec.clone(), json!({})),
                };
                s.run(&id, &params).map_err(|e| format!("{id}: {e}"))?;
            }
            let mut opts = wordcraft_zotero::client::Options::default();
            if let Some(p) = arg_value(&rest, "--port") {
                opts.addr.set_port(p.parse().map_err(|_| format!("bad port `{p}`"))?);
            }
            let trace = rest.iter().any(|a| a == "--trace");
            let mut bridge = wordcraft_zotero::Bridge::new();
            let mut host = wordcraft_zotero::Headless::default();
            let out = wordcraft_zotero::client::run_command(&opts, command, &mut |call| {
                let r = bridge.handle(&mut s, &mut host, call);
                if trace {
                    eprintln!("← {} {}", call.method, clip(&Value::Array(call.args.clone()).to_string()));
                    eprintln!("→ {}", clip(&String::from_utf8_lossy(&wordcraft_zotero::wire::encode_reply(&r))));
                }
                r
            })
            .map_err(|e| e.to_string())?;
            for a in &host.alerts {
                eprintln!("Zotero: {a}");
            }
            eprintln!("{} calls, {}", out.calls, if out.completed { "completed" } else { "ended without completing (cancelled?)" });
            if let Some(o) = arg_value(&rest, "--save") {
                let r = s.run("file.save", &json!({"path": o})).map_err(|e| e.to_string())?;
                eprintln!("{}", wrote(&o, &r));
            }
            Ok(())
        }
        "--version" | "-V" => write_stdout(|o| writeln!(o, "etch-cli {}", env!("CARGO_PKG_VERSION"))),
        "" | "help" | "--help" | "-h" => write_stdout(|o| write!(o, "{USAGE}")),
        other => Err(format!("unknown command `{other}`\n\n{USAGE}").into()),
    }
}

/// At most 400 characters of `s`, for traces.
fn clip(s: &str) -> String {
    match s.char_indices().nth(400) {
        Some((i, _)) => format!("{}…", s.get(..i).unwrap_or(s)),
        None => s.to_string(),
    }
}

fn parity_markdown(p: &Value) -> String {
    let mut s = String::from(
        "# Etch feature parity\n\nGenerated by `cargo xtask parity` (`etch-cli parity --markdown`): the word-processor feature catalog (`crates/engine/src/catalog.rs`) compared with the live command registry.\n\n",
    );
    let out_of_scope = p["outOfScope"].as_array().cloned().unwrap_or_default();
    s += &format!("**{} of {} catalog features have commands ({}%).**", p["live"], p["total"], p["percent"]);
    if !out_of_scope.is_empty() {
        s += &format!(" {} more are out of scope by design and count in neither number ([below](#out-of-scope)).", out_of_scope.len());
    }
    s += "\n\n| Tab | Live | Total |\n|---|---|---|\n";
    for t in p["tabs"].as_array().cloned().unwrap_or_default() {
        s += &format!("| {} | {} | {} |\n", t["tab"].as_str().unwrap_or(""), t["live"], t["total"]);
    }
    s += "\n## Missing\n\n";
    for t in p["tabs"].as_array().cloned().unwrap_or_default() {
        let m = t["missing"].as_array().cloned().unwrap_or_default();
        if m.is_empty() {
            continue;
        }
        s += &format!("### {}\n\n", t["tab"].as_str().unwrap_or(""));
        for x in m {
            s += &format!("- {}\n", x.as_str().unwrap_or(""));
        }
        s += "\n";
    }
    if !out_of_scope.is_empty() {
        s += "## Out of scope\n\nFeatures of the incumbent that Etch leaves out on purpose. They stay in the catalog, so it lists the incumbent's features honestly, but count in neither the live features nor the total.\n\n";
        // Grouped by tab and reason, in catalog order.
        let field = |x: &Value, k: &str| x[k].as_str().unwrap_or("").to_string();
        let mut heading = (String::new(), String::new());
        for x in &out_of_scope {
            let this = (field(x, "tab"), field(x, "reason"));
            if this != heading {
                if !heading.0.is_empty() {
                    s += "\n";
                }
                s += &format!("### {} ({})\n\n", this.0, this.1);
                heading = this;
            }
            s += &format!("- {} › {} (`{}`)\n", field(x, "group"), field(x, "feature"), field(x, "id"));
        }
        s += "\n";
    }
    s
}

fn main() -> ExitCode {
    // Dates shown are local time (#1707): Insert › Date & Time, DATE fields refreshed on open.
    wordcraft_engine::clock::set_local_zone(wordcraft_engine::clock::system_zone());
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match run(&args) {
        Ok(()) | Err(Failure::StdoutClosed) => ExitCode::SUCCESS,
        Err(Failure::Error(e)) => {
            eprintln!("etch-cli: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }

    #[test]
    fn validators_accept_only_the_documented_options() {
        assert!(validate_options("info", &args(&["file.docx"]), PASSWORD_OPTIONS).is_ok());
        assert!(validate_options("render", &args(&["in.docx", "out.png", "--page", "2", "--scale", "1.5"]), RENDER_OPTIONS).is_ok());
        assert!(validate_options("run", &args(&["--cmd", "text.insert={}", "--cmd", "edit.undo", "--print"]), RUN_OPTIONS).is_ok());
        assert!(validate_options("commands", &args(&["--json"]), COMMANDS_OPTIONS).is_ok());
        assert!(validate_options("parity", &args(&["--markdown"]), PARITY_OPTIONS).is_ok());
        assert!(validate_options("render", &args(&["in.docx", "out.png", "--page"]), RENDER_OPTIONS).is_ok());
        assert!(
            validate_options(
                "zotero",
                &args(&["refresh", "in.docx", "--cmd", "caret.docEnd", "--save", "out.docx", "--trace", "--port", "23119"]),
                ZOTERO_OPTIONS
            )
            .is_ok()
        );
    }

    #[test]
    fn an_option_value_is_never_read_as_a_path() {
        // `--password`'s value sits between the paths and would be taken for one without
        // `positionals`, so `render` would write its picture over a file called after the password.
        assert_eq!(positionals(&args(&["in.docx", "--password", "secret", "out.png"]), RENDER_OPTIONS), vec!["in.docx", "out.png"]);
        assert_eq!(positionals(&args(&["in.docx", "out.png", "--page", "2"]), RENDER_OPTIONS), vec!["in.docx", "out.png"]);
        // A bare flag takes no value, so the next argument stays a path.
        assert_eq!(positionals(&args(&["--print", "in.docx"]), RUN_OPTIONS), vec!["in.docx"]);
        // `convert` is the one with a value-taking flag between the two paths.
        assert_eq!(positionals(&args(&["in.docx", "--format", "flatopc", "out.docx"]), CONVERT_OPTIONS), vec!["in.docx", "out.docx"]);
    }

    #[test]
    fn the_password_comes_from_an_option_a_file_or_the_environment() {
        assert_eq!(password_from(&args(&["f.docx"])), Ok(None));
        assert_eq!(password_from(&args(&["f.docx", "--password", "s3cret"])), Ok(Some("s3cret".into())));
        // `--password` wins over the other two sources.
        assert_eq!(password_from(&args(&["--password", "a", "--password-file", "/nonexistent"])), Ok(Some("a".into())));
    }

    #[test]
    fn a_password_file_loses_only_its_trailing_newline() {
        let path = std::env::temp_dir().join("wordcraft-cli-password-test.txt");
        std::fs::write(&path, "s3cret\r\n").unwrap();
        let got = password_from(&args(&["f.docx", "--password-file", &path.to_string_lossy()]));
        std::fs::remove_file(&path).ok();
        assert_eq!(got, Ok(Some("s3cret".into())));
    }

    #[test]
    fn a_missing_password_file_is_named_without_the_password() {
        let err = password_from(&args(&["f.docx", "--password-file", "/nonexistent/secret.txt"])).unwrap_err();
        assert!(err.contains("--password-file"), "{err}");
    }

    #[test]
    fn unknown_options_are_rejected_before_file_access() {
        assert_eq!(run(&args(&["info", "missing.docx", "--passwrod", "x"])), Err("info: unknown option --passwrod".into()));
        assert_eq!(run(&args(&["render", "missing.docx", "out.png", "--sclae", "2"])), Err("render: unknown option --sclae".into()));
        assert_eq!(run(&args(&["zotero", "refresh", "missing.docx", "--prot", "1"])), Err("zotero: unknown option --prot".into()));
    }

    #[test]
    fn help_or_h_after_a_subcommand_asks_for_the_usage() {
        for cmd in SUBCOMMANDS {
            assert!(wants_help(cmd, &args(&["in.docx", "--help"])), "{cmd}");
            assert!(wants_help(cmd, &args(&["-h"])), "{cmd}");
            assert!(!wants_help(cmd, &args(&["in.docx", "--hepl"])), "{cmd}");
        }
        assert!(!wants_help("frobnicate", &args(&["--help"])));
    }

    #[test]
    fn usage_lists_exactly_the_subcommands() {
        let listed: Vec<&str> = USAGE
            .lines()
            .filter_map(|line| line.strip_prefix("  etch-cli ")?.split_whitespace().next())
            .filter(|word| !word.starts_with(['-', '[']))
            .collect();
        assert_eq!(listed, SUBCOMMANDS);
    }

    #[test]
    fn text_keeps_bullet_and_number_labels() {
        let doc = wordcraft_engine::io::open_bytes("lists.md", b"- alpha\n- beta\n\n1. first\n2. second\n").unwrap();
        assert_eq!(listed_text(&doc).unwrap(), "\u{2022}\talpha\n\u{2022}\tbeta\n1.\tfirst\n2.\tsecond");
    }

    #[test]
    fn author_flag_names_the_headless_session() {
        assert!(validate_options("run", &args(&["--author", "Claude (copyedit)", "--cmd", "edit.undo"]), RUN_OPTIONS).is_ok());
        let mut s = Session::new(wordcraft_doc::Document::new());
        assert!(set_author(&mut s, " Claude (copyedit) ").is_ok());
        assert_eq!(s.author, "Claude (copyedit)");
        assert!(set_author(&mut s, "   ").is_err_and(|e| e.starts_with("--author:")));
        assert!(run(&args(&["run", "--author", "Claude", "--cmd", "review.trackChanges={\"value\":true}"])).is_ok());
    }

    #[test]
    fn cli_open_retains_native_file_identity_for_saves() {
        let dir = std::env::temp_dir().join(format!("wordcraft-cli-conflict-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("input.docx");
        wordcraft_engine::io::save_path(&path, &wordcraft_doc::Document::from_text("Original A")).unwrap();
        let mut s = open(&path, None).unwrap();
        s.run("document.setText", &json!({"text": "Local edit"})).unwrap();
        let external = wordcraft_engine::io::save_bytes("external.docx", &wordcraft_doc::Document::from_text("External B")).unwrap();
        std::fs::write(&path, &external).unwrap();
        assert!(matches!(s.run("file.save", &json!({})), Err(wordcraft_engine::CmdError::FileConflict(_))));
        assert!(s.dirty);
        assert_eq!(std::fs::read(&path).unwrap(), external);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A conversion to a format that can't hold all of the document (comments and footnotes in
    /// plain text) says what the file is without; one that keeps everything says only where it
    /// wrote. Synthetic text, a temporary folder.
    #[test]
    fn a_lossy_save_says_what_the_file_is_without() {
        let dir = std::env::temp_dir().join(format!("wordcraft-cli-lossy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        let mut s = session(wordcraft_doc::Document::from_text("Synthetic body text"));
        s.run("review.newComment", &json!({"text": "Synthetic comment"})).unwrap();
        s.run("references.footnote", &json!({"text": "Synthetic footnote"})).unwrap();
        let txt = dir.join("out.txt").to_string_lossy().into_owned();
        let said = save_to(&mut s, &txt, json!({})).unwrap();
        assert!(said.starts_with(&format!("wrote {txt}")), "{said}");
        assert!(said.contains("warning") && said.contains("comments") && said.contains("footnotes"), "{said}");
        let docx = dir.join("out.docx").to_string_lossy().into_owned();
        assert_eq!(save_to(&mut s, &docx, json!({})).unwrap(), format!("wrote {docx}"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// `convert m.docm m.docx` (and `.dotm` → `.dotx`): the macro-free file is without the macro
    /// project, and stderr says so; a `.docm` keeps it and says only where it wrote (06, question 2).
    #[test]
    fn a_conversion_that_leaves_out_the_macros_says_so() {
        let dir = std::env::temp_dir().join(format!("wordcraft-cli-macros-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        let mut doc = wordcraft_doc::Document::from_text("Synthetic macro document");
        doc.passthrough.insert("word/vbaProject.bin".into(), std::sync::Arc::new(vec![0xD0, 0xCF, 0x11, 0xE0, 9, 9]));
        for (source, target) in [("in.docm", "out.docx"), ("in.dotm", "out.dotx")] {
            let src = dir.join(source);
            wordcraft_engine::io::save_path(&src, &doc).unwrap();
            let mut s = session(wordcraft_engine::io::open_path(&src).unwrap());
            let out = dir.join(target).to_string_lossy().into_owned();
            let said = save_to(&mut s, &out, json!({})).unwrap();
            assert!(said.starts_with(&format!("wrote {out}")), "{said}");
            assert!(said.contains("warning") && said.contains("macros"), "{target}: {said}");
            let kept = dir.join("again.docm").to_string_lossy().into_owned();
            assert_eq!(save_to(&mut s, &kept, json!({})).unwrap(), format!("wrote {kept}"));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn only_the_opened_file_may_be_other_than_unicode() {
        use std::os::unix::ffi::OsStringExt;
        let latin1 = || OsString::from_vec(b"caf\xe9.docx".to_vec());
        let ok = |raw: Vec<OsString>| text_args(&raw).unwrap().1;
        assert_eq!(ok(vec!["zotero".into(), "refresh".into(), latin1()]), Some(PathBuf::from(latin1())));
        assert_eq!(ok(vec!["render".into(), latin1(), "out.png".into(), "--page".into(), "2".into()]), Some(PathBuf::from(latin1())));
        assert_eq!(ok(vec!["run".into(), "--print".into(), "--file".into(), latin1()]), Some(PathBuf::from(latin1())));
        assert_eq!(ok(vec!["info".into(), "in.docx".into()]), None);
        // Every command that opens a file reaches the open with it (no such file here), so
        // `OPENS_FILE` and the commands' own arguments agree.
        let missing = || OsString::from_vec(b"/definitely/not/caf\xe9.docx".to_vec());
        let mut opening: Vec<Vec<OsString>> = OPENS_FILE
            .iter()
            .map(|(cmd, _, n)| [vec![(*cmd).into()], vec![OsString::from("refresh"); *n], vec![missing(), "out.png".into()]].concat())
            .collect();
        opening.push(vec!["run".into(), "--file".into(), missing()]);
        for raw in opening {
            let e = run(&raw).unwrap_err().to_string();
            assert!(e.contains("No such file"), "{raw:?}: {e}");
        }
        for (raw, n) in [
            (vec!["zotero".into(), latin1(), "in.docx".into()], 2),
            (vec!["render".into(), "in.docx".into(), latin1()], 3),
            (vec!["run".into(), "--save".into(), latin1()], 3),
            (vec!["commands".into(), latin1()], 2),
        ] {
            assert_eq!(text_args(&raw).unwrap_err(), format!("argument {n} is not valid Unicode: caf\u{FFFD}.docx"));
        }
    }

    #[test]
    fn option_values_are_not_positionals() {
        let a = args(&["in.docx", "--page", "2", "out.png", "--scale", "1.5"]);
        assert_eq!(positionals(&a, RENDER_OPTIONS), vec!["in.docx", "out.png"]);
        let a = args(&["--markup", "in.docx", "out.pdf"]);
        assert_eq!(positionals(&a, CONVERT_OPTIONS), vec!["in.docx", "out.pdf"]);
        let a = args(&["--cmd", "caret.docEnd", "refresh", "--port", "1", "in.docx"]);
        assert_eq!(positionals(&a, ZOTERO_OPTIONS), vec!["refresh", "in.docx"]);
        // `render in.docx --page 2` names no output: an error, not a file called "2".
        assert!(run(&args(&["render", "in.docx", "--page", "2"])).is_err_and(|e| e.to_string().contains("missing argument")));
    }

    #[cfg(unix)]
    #[test]
    fn the_opened_file_is_found_past_options_that_take_a_value() {
        // Pure argument handling, no file system: runs on macOS too, whose APFS refuses such names.
        use std::os::unix::ffi::OsStringExt;
        let latin1 = || OsString::from_vec(b"caf\xe9.docx".to_vec());
        let file = |raw: Vec<OsString>| text_args(&raw).map(|(_, f)| f);
        // `--page 2` comes first: the file is the first positional, not the first non-flag.
        assert_eq!(file(vec!["render".into(), "--page".into(), "2".into(), latin1(), "out.png".into()]), Ok(Some(PathBuf::from(latin1()))));
        assert_eq!(file(vec!["zotero".into(), "--port".into(), "1".into(), "refresh".into(), latin1()]), Ok(Some(PathBuf::from(latin1()))));
        assert_eq!(file(vec!["zotero".into(), "--cmd".into(), "caret.docEnd".into(), "refresh".into(), latin1()]), Ok(Some(PathBuf::from(latin1()))));
        // The value of an option is never taken for the file.
        assert!(file(vec!["render".into(), "--page".into(), latin1(), "in.docx".into(), "out.png".into()]).is_err());
        assert!(file(vec!["info".into(), OsString::from_vec(vec![b'a', 0xff])]).is_ok());
        assert!(file(vec![OsString::from_vec(vec![b'a', 0xff])]).is_err_and(|e| e.contains("argument 1 is not valid Unicode")));
    }

    #[test]
    fn render_rejects_missing_or_invalid_page_and_scale() {
        // Missing operands
        assert_eq!(run(&args(&["render", "missing.docx", "out.png", "--page"])), Err("render: --page requires a value".into()));
        assert_eq!(run(&args(&["render", "missing.docx", "out.png", "--scale"])), Err("render: --scale requires a value".into()));
        assert_eq!(run(&args(&["render", "missing.docx", "out.png", "--page", "--scale", "2"])), Err("render: --page requires a value".into()));
        assert_eq!(run(&args(&["render", "missing.docx", "out.png", "--scale", "--page", "1"])), Err("render: --scale requires a value".into()));

        // Non-numeric operands
        assert_eq!(
            run(&args(&["render", "missing.docx", "out.png", "--page", "banana"])),
            Err("render: invalid --page `banana`: not a valid integer".into())
        );
        assert_eq!(
            run(&args(&["render", "missing.docx", "out.png", "--scale", "banana"])),
            Err("render: invalid --scale `banana`: not a number".into())
        );

        // Out-of-range values
        assert_eq!(
            run(&args(&["render", "missing.docx", "out.png", "--page", "0"])),
            Err("render: invalid --page `0`: page numbers start at 1".into())
        );
        assert_eq!(
            run(&args(&["render", "missing.docx", "out.png", "--page", "-1"])),
            Err("render: invalid --page `-1`: not a valid integer".into())
        );
        assert_eq!(
            run(&args(&["render", "missing.docx", "out.png", "--scale", "0"])),
            Err("render: invalid --scale `0`: scale must be a positive number".into())
        );
        assert_eq!(
            run(&args(&["render", "missing.docx", "out.png", "--scale", "-2"])),
            Err("render: invalid --scale `-2`: scale must be a positive number".into())
        );
        assert_eq!(
            run(&args(&["render", "missing.docx", "out.png", "--scale", "NaN"])),
            Err("render: invalid --scale `NaN`: scale must be a positive number".into())
        );
        assert_eq!(
            run(&args(&["render", "missing.docx", "out.png", "--scale", "inf"])),
            Err("render: invalid --scale `inf`: scale must be a positive number".into())
        );

        // Duplicate options
        assert_eq!(run(&args(&["render", "missing.docx", "out.png", "--page", "1", "--page", "2"])), Err("render: duplicate --page option".into()));
        assert_eq!(
            run(&args(&["render", "missing.docx", "out.png", "--scale", "1", "--scale", "2"])),
            Err("render: duplicate --scale option".into())
        );
    }

    #[test]
    fn run_rejects_missing_or_invalid_page_and_scale() {
        // Missing operands
        assert_eq!(run(&args(&["run", "--template", "sample", "--save", "out.png", "--page"])), Err("run: --page requires a value".into()));
        assert_eq!(run(&args(&["run", "--template", "sample", "--save", "out.png", "--scale"])), Err("run: --scale requires a value".into()));
        assert_eq!(
            run(&args(&["run", "--template", "sample", "--save", "out.png", "--page", "--scale", "2"])),
            Err("run: --page requires a value".into())
        );
        assert_eq!(
            run(&args(&["run", "--template", "sample", "--save", "out.png", "--scale", "--page", "1"])),
            Err("run: --scale requires a value".into())
        );

        // Non-numeric operands
        assert_eq!(
            run(&args(&["run", "--template", "sample", "--save", "out.png", "--page", "banana"])),
            Err("run: invalid --page `banana`: not a valid integer".into())
        );
        assert_eq!(
            run(&args(&["run", "--template", "sample", "--save", "out.png", "--scale", "banana"])),
            Err("run: invalid --scale `banana`: not a number".into())
        );

        // Out-of-range values
        assert_eq!(
            run(&args(&["run", "--template", "sample", "--save", "out.png", "--page", "0"])),
            Err("run: invalid --page `0`: page numbers start at 1".into())
        );
        assert_eq!(
            run(&args(&["run", "--template", "sample", "--save", "out.png", "--page", "-1"])),
            Err("run: invalid --page `-1`: not a valid integer".into())
        );
        assert_eq!(
            run(&args(&["run", "--template", "sample", "--save", "out.png", "--scale", "0"])),
            Err("run: invalid --scale `0`: scale must be a positive number".into())
        );
        assert_eq!(
            run(&args(&["run", "--template", "sample", "--save", "out.png", "--scale", "-1"])),
            Err("run: invalid --scale `-1`: scale must be a positive number".into())
        );
        assert_eq!(
            run(&args(&["run", "--template", "sample", "--save", "out.png", "--scale", "NaN"])),
            Err("run: invalid --scale `NaN`: scale must be a positive number".into())
        );
        assert_eq!(
            run(&args(&["run", "--template", "sample", "--save", "out.png", "--scale", "inf"])),
            Err("run: invalid --scale `inf`: scale must be a positive number".into())
        );

        // Duplicate options
        assert_eq!(
            run(&args(&["run", "--template", "sample", "--save", "out.png", "--page", "1", "--page", "2"])),
            Err("run: duplicate --page option".into())
        );
        assert_eq!(
            run(&args(&["run", "--template", "sample", "--save", "out.png", "--scale", "1", "--scale", "2"])),
            Err("run: duplicate --scale option".into())
        );
    }

    #[test]
    fn render_and_run_valid_options_and_defaults_produce_files() {
        let dir = std::env::temp_dir().join(format!("wc-cli-render-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let docx_path = dir.join("sample.docx");
        // Create sample docx via `run --save sample.docx`
        assert!(run(&args(&["run", "--template", "sample", "--save", &docx_path.to_string_lossy()])).is_ok());
        assert!(docx_path.exists());

        let docx_str = docx_path.to_string_lossy().into_owned();

        // 1. render with defaults
        let out_default = dir.join("out_default.png");
        assert!(run(&args(&["render", &docx_str, &out_default.to_string_lossy()])).is_ok());
        assert!(out_default.exists());

        // 2. render with valid --page and --scale
        let out_valid = dir.join("out_valid.png");
        assert!(run(&args(&["render", &docx_str, &out_valid.to_string_lossy(), "--page", "1", "--scale", "2"])).is_ok());
        assert!(out_valid.exists());

        // 3. render with different flag order
        let out_reordered = dir.join("out_reordered.png");
        assert!(run(&args(&["render", &docx_str, &out_reordered.to_string_lossy(), "--scale", "1", "--page", "1"])).is_ok());
        assert!(out_reordered.exists());

        // 4. render failure leaves no output file
        let out_invalid = dir.join("out_invalid.png");
        assert!(run(&args(&["render", &docx_str, &out_invalid.to_string_lossy(), "--page", "banana"])).is_err());
        assert!(!out_invalid.exists());

        // 5. run --save out.png with defaults
        let run_default = dir.join("run_default.png");
        assert!(run(&args(&["run", "--file", &docx_str, "--save", &run_default.to_string_lossy()])).is_ok());
        assert!(run_default.exists());

        // 6. run --save out.png with --page and --scale
        let run_valid = dir.join("run_valid.png");
        assert!(run(&args(&["run", "--file", &docx_str, "--save", &run_valid.to_string_lossy(), "--page", "1", "--scale", "1"])).is_ok());
        assert!(run_valid.exists());

        // 7. run --save out.png with different flag order
        let run_reordered = dir.join("run_reordered.png");
        assert!(run(&args(&["run", "--file", &docx_str, "--scale", "1", "--page", "1", "--save", &run_reordered.to_string_lossy()])).is_ok());
        assert!(run_reordered.exists());

        // 8. run --save out.png failure leaves no output file
        let run_invalid = dir.join("run_invalid.png");
        assert!(run(&args(&["run", "--file", &docx_str, "--save", &run_invalid.to_string_lossy(), "--scale", "banana"])).is_err());
        assert!(!run_invalid.exists());

        let _ = std::fs::remove_dir_all(dir);
    }

    /// `docs/parity-checklist.md` lists out-of-scope features in their own section, not as
    /// missing, and says the counts leave them out.
    #[test]
    fn parity_markdown_lists_out_of_scope_features_apart() {
        let p = json!({
            "live": 1, "total": 2, "percent": 50.0,
            "tabs": [{"tab": "Mailings", "live": 1, "total": 2, "missing": ["Create › Labels (`mailings.labels`)"]}],
            "outOfScope": [
                {"tab": "Mailings", "group": "Finish", "feature": "Finish & Merge", "id": "mailings.finish", "reason": "Etch has no mail merge"},
                {"tab": "Mailings", "group": "Preview Results", "feature": "Next Record", "id": "mailings.next", "reason": "Etch has no mail merge"},
            ],
        });
        let md = parity_markdown(&p);
        assert!(md.contains("**1 of 2 catalog features have commands (50.0%).** 2 more are out of scope by design"), "{md}");
        let (missing, out) = md.split_once("## Out of scope").unwrap();
        assert!(missing.contains("mailings.labels") && !missing.contains("mailings.finish"), "{md}");
        assert_eq!(out.matches("### Mailings (Etch has no mail merge)").count(), 1, "{md}");
        assert!(out.contains("- Finish › Finish & Merge (`mailings.finish`)\n- Preview Results › Next Record (`mailings.next`)\n"), "{md}");
        assert!(!parity_markdown(&json!({"live": 0, "total": 0, "percent": 0.0, "tabs": []})).contains("Out of scope"));
    }

    /// A scratch folder that removes itself, even when the test fails.
    struct ScriptDir(PathBuf);

    impl std::ops::Deref for ScriptDir {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for ScriptDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn script_dir(tag: &str) -> ScriptDir {
        let dir = std::env::temp_dir().join(format!("wc-cli-script-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        ScriptDir(dir)
    }

    fn write_script(dir: &Path, text: &str) -> String {
        let p = dir.join("job.json");
        std::fs::write(&p, text).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn script_builds_a_document_and_saves_docx_and_pdf_relative_to_the_script() {
        let dir = script_dir("two");
        let script = write_script(
            &dir,
            r#"{"template":"blank","commands":[{"command":"text.insert","params":{"text":"Hello script"}},{"command":"format.bold"}],"save":["out.docx","out.pdf"]}"#,
        );
        run(&args(&["run", "--script", &script])).unwrap();
        assert!(std::fs::read(dir.join("out.pdf")).unwrap().starts_with(b"%PDF"));
        let doc = open(&dir.join("out.docx"), None).unwrap().doc;
        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("Hello script") && json.contains("\"bold\":true"));
    }

    #[test]
    fn script_with_a_bad_output_writes_no_output() {
        let dir = script_dir("badout");
        let script = write_script(&dir, r#"{"commands":["caret.docEnd"],"save":["a.docx","b.xyz"]}"#);
        let e = run(&args(&["run", "--script", &script])).unwrap_err().to_string();
        assert_eq!(e.matches("b.xyz").count(), 1, "{e}");
        assert!(!dir.join("a.docx").exists());
        let script = write_script(&dir, r#"{"commands":["caret.docEnd"],"save":["a.docx","nope/b.docx"]}"#);
        assert!(run(&args(&["run", "--script", &script])).is_err());
        assert!(!dir.join("a.docx").exists());
    }

    #[test]
    fn script_with_a_folder_as_output_writes_no_output() {
        let dir = script_dir("folderout");
        std::fs::create_dir(dir.join("b.pdf")).unwrap();
        let script = write_script(&dir, r#"{"commands":["caret.docEnd"],"save":["a.docx","b.pdf"]}"#);
        let e = run(&args(&["run", "--script", &script])).unwrap_err().to_string();
        assert!(e.contains("b.pdf"), "{e}");
        assert!(!dir.join("a.docx").exists());
    }

    /// APFS refuses file names that aren't valid UTF-8, so the fixture can't be made on macOS
    /// (as in `tests/non_utf8_args.rs`).
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn script_file_name_need_not_be_unicode() {
        use std::os::unix::ffi::OsStringExt;
        let dir = script_dir("latin1");
        let mut name = dir.as_os_str().to_os_string().into_vec();
        name.extend_from_slice(b"/caf\xe9.json");
        let path = PathBuf::from(OsString::from_vec(name));
        std::fs::write(&path, r#"{"commands":["caret.docEnd"],"save":["o.txt"]}"#).unwrap();
        let raw = vec![OsString::from("run"), OsString::from("--script"), path.into_os_string()];
        run(&raw).unwrap();
        assert!(dir.join("o.txt").exists());
    }

    #[test]
    fn command_line_template_wins_over_the_scripts_file() {
        let dir = script_dir("source");
        let input = dir.join("in.docx");
        run(&args(&["run", "--template", "blank", "--cmd", "text.insert={\"text\":\"FROM FILE\"}", "--save", &input.to_string_lossy()])).unwrap();
        let script = write_script(&dir, r#"{"file":"in.docx","commands":["caret.docEnd"]}"#);
        let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
        run(&args(&["run", "--script", &script, "--template", "blank", "--save", &a.to_string_lossy()])).unwrap();
        assert!(!std::fs::read_to_string(&a).unwrap().contains("FROM FILE"));
        run(&args(&["run", "--script", &script, "--save", &b.to_string_lossy()])).unwrap();
        assert!(std::fs::read_to_string(&b).unwrap().contains("FROM FILE"));
    }

    #[test]
    fn script_value_missing_is_reported() {
        let e = run(&args(&["run", "--script", "--cmd", "caret.docEnd"])).unwrap_err().to_string();
        assert!(e.contains("--script requires a value"), "{e}");
    }

    #[test]
    fn script_reads_from_a_stream_and_caps_its_size() {
        let s = script::load_from(&br#"{"commands":["caret.docEnd"],"save":["o.docx"]}"#[..], "-", None).unwrap();
        assert_eq!((s.commands.len(), s.save), (1, vec!["o.docx".to_string()]));
        let big = std::io::repeat(b' ').take(script::MAX_SCRIPT_BYTES + 10);
        let e = script::load_from(big, "-", None).unwrap_err();
        assert!(e.contains("too large"), "{e}");
        assert!(script::load_from(&b"{"[..], "-", None).is_err());
    }

    #[test]
    fn script_commands_run_before_cmd_flags() {
        let dir = script_dir("order");
        let script = write_script(&dir, r#"{"commands":[{"command":"text.insert","params":{"text":"first"}}]}"#);
        let out = dir.join("o.txt");
        run(&args(&["run", "--script", &script, "--cmd", "text.insert={\"text\":\" second\"}", "--save", &out.to_string_lossy()])).unwrap();
        assert!(std::fs::read_to_string(&out).unwrap().contains("first second"));
    }

    #[test]
    fn script_failure_names_the_command_and_writes_nothing() {
        let dir = script_dir("fail");
        let script =
            write_script(&dir, r#"{"commands":[{"command":"text.insert","params":{"text":"x"}},{"command":"no.such.command"}],"save":["out.docx"]}"#);
        let e = run(&args(&["run", "--script", &script])).unwrap_err().to_string();
        assert!(e.contains("command 2") && e.contains("no.such.command"), "{e}");
        assert!(!dir.join("out.docx").exists());
    }

    #[test]
    fn script_rejects_junk_empty_missing_and_huge_input() {
        let dir = script_dir("junk");
        for text in [
            "not json",
            "[]",
            "{}",
            r#"{"commands":[]}"#,
            r#"{"commands":[{"params":{}}]}"#,
            r#"{"commands":[1]}"#,
            r#"{"commands":[{"command":"a"}],"save":[1]}"#,
        ] {
            let script = write_script(&dir, text);
            assert!(run(&args(&["run", "--script", &script])).is_err(), "{text}");
        }
        assert!(run(&args(&["run", "--script", &dir.join("missing.json").to_string_lossy()])).is_err());
        let huge = write_script(&dir, &" ".repeat(5 * 1024 * 1024));
        let e = run(&args(&["run", "--script", &huge])).unwrap_err().to_string();
        assert!(e.contains("too large"), "{e}");
    }

    #[test]
    fn script_parse_resolves_paths_against_the_base() {
        let s = script::parse(r#"{"file":"in.docx","commands":[{"command":"edit.undo"}],"save":["/abs/o.pdf","o.docx"]}"#, Some(Path::new("/base")))
            .unwrap();
        assert_eq!(s.file.as_deref(), Some("/base/in.docx"));
        assert_eq!(s.save, vec!["/abs/o.pdf".to_string(), "/base/o.docx".to_string()]);
        let s = script::parse(r#"{"commands":["caret.docEnd"],"save":["o.docx"]}"#, None).unwrap();
        assert_eq!(s.save, vec!["o.docx".to_string()]);
        assert_eq!(s.commands.len(), 1);
    }
}
