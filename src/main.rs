#![cfg_attr(not(test), windows_subsystem = "windows")]
//! RustExplorer — a two-click Windows 11 style file manager with a preview pane.
use rx_core::{col_widths, ext_of, i18n::plural, t, fmt_attrs, fmt_size, fmt_time, glob_any, group, icon, kind_of, natural_cmp, Kind, APP_NAME, IMAGE_EXT, TEXT_EXT};
use rx_sys as sys;
use rx_update as update;

// structs and enums stay here so their fields are visible to every module below
mod actions;
mod archive;
mod bars;
mod list;
mod listing;
mod ops;
mod preview;
mod scan;
mod theme;
mod tree;
mod tray;
mod treemap;
use listing::*;
use preview::*;
use scan::*;
use theme::*;
use tray::*;
use treemap::*;

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AO};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime};

use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, ColorImage, CornerRadius, FontFamily, FontId, Key, Layout, Margin,
    Modifiers, Painter, Rect, Response, RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui, UiBuilder,
};

// colours: pal() in theme.rs (rx_core::palette). Green text (new_text) marks files created or
// changed in the last 3 hours (RECENT_SECS); the owner relies on it.
/// the window's first size, and the least it may have (points). The least width is where the app
/// icon, the four menus and the caption buttons still fit side by side in the title bar; snap
/// layouts need it at 500 or less (Microsoft's rule). The height fits a quarter of a 1280×720 pt screen.
const START_SIZE: [f32; 2] = [1600.0, 860.0];
const MIN_SIZE: [f32; 2] = [420.0, 320.0];
/// Window widths under which the tree, and the preview, are left out (points).
const NARROW: [f32; 2] = [480.0, 700.0];
/// Column widths (Wielkość, Typ, Zmodyfikowany, Atrybuty) a column gets when shown again. The date
/// fits the longest English one, "12/31/2026 11:59 PM" (the Polish one is shorter).
const DEFAULT_W: [f32; 4] = [90.0, 210.0, 160.0, 80.0];
/// How long a file counts as new / recently changed: 3 hours (owner's setting).
const RECENT_SECS: u64 = 3 * 60 * 60;
/// Pseudo-path of the "Ten komputer" (drives) view.
const PC: &str = "::pc";
/// Drive bar turns red above this share used (Windows Explorer's rule).
const DRIVE_RED: f32 = 0.9;
/// Narrowest the Nazwa column can be dragged.
const MIN_NAME_W: f32 = 120.0;

fn is_pc(p: &Path) -> bool {
    p.as_os_str() == PC
}

/// When main() began, for a test run's time to its first frame.
static STARTED: OnceLock<Instant> = OnceLock::new();

/// "40 min temu" for a modification within RECENT_SECS, None for older ones. Green rows also count
/// a recent creation (a copied old file), which keeps its real modification date here.
fn ago(t: Option<SystemTime>) -> Option<String> {
    let secs = SystemTime::now().duration_since(t?).ok()?.as_secs();
    (secs < RECENT_SECS).then(|| rx_core::fmt_ago(secs))
}

// ---------- listing ----------
#[derive(Clone)]
struct Entry {
    name: String,
    path: PathBuf,
    dir: bool,
    size: Option<u64>,
    modified: Option<SystemTime>,
    attrs: u32,
    kind: Kind,
    type_name: Arc<str>,
    /// created or modified within RECENT_SECS
    recent: bool,
    /// "Analiza miejsca": how deep in the folder's tree (0: the folder's own items, listed live;
    /// deeper rows come from the index under an expanded folder)
    depth: u8,
}

#[derive(Clone, Copy, PartialEq)]
enum Col {
    Name,
    Size,
    Type,
    Modified,
    Attr,
}

struct Tab {
    path: PathBuf,
    back: Vec<PathBuf>,
    fwd: Vec<PathBuf>,
    all: Vec<Entry>,
    view: Vec<usize>,
    hidden: usize,
    sel: HashSet<PathBuf>,
    /// the name filter (*.pdf;*.jpg) of this tab only, kept across its folders
    filter: String,
    /// this tab's search box text, and whether it searches every local drive (the whole-drive
    /// index) instead of this folder; both end when the tab opens another folder
    search: String,
    search_all: bool,
    anchor: Option<usize>,
    focus: Option<PathBuf>,
    sort: Col,
    asc: bool,
    flat: bool,
    /// the list shows "szukaj wszędzie" results, not the folder: for which query and index
    /// (App::index_gen), and how many names matched in all
    found: bool,
    found_for: (String, u64),
    found_total: usize,
    /// "Analiza miejsca": the list holds the biggest files under this folder (found is set too)
    biggest: bool,
    /// "Analiza miejsca": the index generation the folders' sizes were filled from (u64::MAX:
    /// counted on the spot, the index does not reach here), and the sorting to give back after
    sized: Option<u64>,
    sort_before_space: Option<(Col, bool)>,
    /// "Analiza miejsca": folders opened in place in the list's tree
    expanded: HashSet<PathBuf>,
    error: Option<String>,
    dirty: bool,
    /// a results list drops rows that are gone from disk at its next refresh_view, not at once:
    /// the view in use until then holds indices into the rows as they are
    prune: bool,
    scroll_to: Option<usize>,
    /// the list's scroll offset and viewport height in the last frame
    scroll: (f32, f32),
    /// which pane of the split view this tab belongs to (0 left, 1 right)
    side: u8,
    /// the folder is an archive's (see archive.rs): by its name until its rows come, then as read
    in_arc: bool,
    /// the rows asked of a worker thread and what to select once they are in (listing.rs)
    asked: rx_core::Asked<rx_core::Select>,
    /// the rows (or the error) are of the folder in `path`, however old; false until its first come
    shown: bool,
    /// that request is still to be given to a thread (App::send_listings); when it was made
    unsent: bool,
    asked_at: Instant,
    /// the rows on their way follow a change by another program: what the list knew stays (carry_over)
    carry: bool,
}

// ---------- preview and background work ----------
enum Raw {
    Image(ColorImage, String),
    Text(String),
    Folder(usize, usize, u64),
    Nothing(String),
    /// a file that is only in the cloud, not read: the shell's thumbnail if it has one, and
    /// whether its content would show once downloaded (a picture, a PDF, text)
    Cloud(Option<ColorImage>, bool),
}

enum Shown {
    Loading,
    Image(TextureHandle, String),
    Text(String),
    Folder(usize, usize, u64),
    Nothing(String),
    Cloud(Option<TextureHandle>, bool),
}

/// The shell's picture of a file in the list: `tex` is None until it comes and when there is none.
struct Thumb {
    tex: Option<TextureHandle>,
    /// the frame that drew it last (see THUMB_CAP)
    seen: u64,
}

/// Thumbnail textures kept at most (64 KB each); over it the longest undrawn go and are asked for
/// again when their tile is drawn.
const THUMB_CAP: usize = 500;

/// A drive in the "Ten komputer" view: (root, name, type, free, total, file system).
type PcRow = (PathBuf, String, u32, u64, u64, String);

enum Msg {
    Preview(PathBuf, Raw),
    /// a list picture
    Thumb(PathBuf, Option<ColorImage>),
    DirSize(PathBuf, u64),
    /// the whole-drive index read from its file, then the end of the background work with the
    /// freshly scanned index (None when no rescan was due)
    Index(Arc<rx_core::index::Index>),
    Indexed(Option<Arc<rx_core::index::Index>>),
    /// a background job ended: what it did or what went wrong, for the status bar
    Note(String),
    /// a file operation ended: what Ctrl+Z can take back of it
    Undo(Undo),
    /// an archive needs its password (true: the one given was wrong; then: unpack it here, not into a folder)
    AskPassword(PathBuf, bool, bool),
    Reload,
    /// a folder's rows, read for the request of this number (Tab::asked); true: it is an archive's
    Listed(u64, Result<Vec<Entry>, String>, bool),
    /// the folders in a folder of the tree for the request of this number (App::tree_asked), and
    /// the hidden ones among them
    Kids(u64, PathBuf, Vec<PathBuf>, Vec<PathBuf>),
    /// free and total bytes of the volume this folder is on
    Disk(PathBuf, u64, u64),
    /// the "Ten komputer" rows
    Pc(Vec<PcRow>),
    /// the places of Widok główny and the cloud folders, read once at start
    Places(Vec<(PathBuf, String, Kind)>, Vec<PathBuf>),
    /// another program changed a folder on screen (true: it cannot be read any more), from the watcher
    Folder(PathBuf, bool),
    /// the drives of these letters (bits as sys::drive_mask) are replaced by the ones given: a
    /// drive came or went, or their names were read (start, F5)
    Drives(u32, Vec<(PathBuf, String, bool)>),
    /// open this path as Enter on it would (a file unpacked from an archive)
    Open(PathBuf),
    /// a file opened from an archive for editing: its copy, the archive, the entry (archive.rs)
    Watch(PathBuf, PathBuf, String),
    /// an archive was changed: what it made, to select (true: rename the first, a new folder)
    Changed(Vec<PathBuf>, bool),
    /// VirusTotal's answer about a file (scan.rs)
    Vt(PathBuf, Vt),
    /// a cloud file's download ended (true: well): its rows read their state again, its preview
    /// is built again
    Fetched(PathBuf, bool),
    /// a file only in the cloud came for Otwórz: opened on this thread (the shell's handlers
    /// want its apartment)
    OpenFetched(PathBuf),
    /// the items for Windows' menu (or why the disk did not give them) asked for at this time, to
    /// be shown at this point in the window (pixels; None: at the pointer)
    ShellMenu(Result<sys::menu::Items, String>, Option<(i32, i32)>, Instant),
}

// ---------- the app ----------
#[derive(Clone, Copy, PartialEq)]
enum ViewMode {
    Details,
    Thumbs,
    DetailsThumbs,
}

#[derive(Clone)]
enum Act {
    NewTab(Option<PathBuf>),
    CloseTab(usize),
    NewFolder,
    Rename,
    Delete,
    Copy(bool),
    Paste,
    SelectAll,
    Invert,
    Open,
    OpenPath(PathBuf),
    Properties,
    MakeBak,
    Extract,
    /// unpack the selected archives straight into their folder, no new folder of their name
    ExtractHere,
    /// ask for the name, format, compression and password of a new archive of the selection (the Spakuj dialog)
    Pack,
    /// pack the selection here into `name`.`kind` ("zip" or "7z") with these options
    PackAs(String, &'static str, rx_archive::PackOpts),
    /// a change to an archive's contents (true: rename what it made); asked first when it deletes
    ArchiveChange(PathBuf, rx_archive::Change, bool),
    /// a RAR made into a ZIP copy, then that copy's folder (the archive and the folder in it)
    ConvertArchive(PathBuf, String),
    Refresh,
    Undo,
    Back,
    Forward,
    Up,
    CalcSizes,
    /// Spacja: the size of the selected folders
    SizeSelected,
    ToggleFlat,
    Goto(PathBuf),
    AddBookmark(PathBuf),
    RemoveBookmark(PathBuf),
    Terminal,
    CopyPath,
    Exit,
    ToggleSplit,
    ToggleHidden,
    /// split view: the other pane becomes the active one
    SwitchPane,
    /// the search box switches to the name filter and takes focus
    FilterNames,
    /// "Analiza miejsca" on or off
    ToggleSpace,
    /// the biggest files under the current folder, or the folder again
    Biggest,
    /// "Analiza miejsca": to the Recycle Bin from the treemap (paths anywhere, not the selection)
    Recycle(Vec<PathBuf>),
    /// the treemap: open the file's folder with the file selected
    Reveal(PathBuf),
    /// "Analiza miejsca": a folder of the list's tree opens or closes in place
    ToggleExpand(PathBuf),
    /// drag and drop: these items into this folder, dropped in the pane of this tab; true: moved
    Drop(Vec<PathBuf>, PathBuf, usize, bool),
    /// a tab dragged to a pane (0 left, 1 right); outside split view it opens the split
    TabToSide(usize, u8),
    /// VirusTotal: check this file by its hash, send it there, or ask for the API key (scan.rs)
    VtCheck(PathBuf),
    VtSend(PathBuf),
    VtKey,
    /// Windows' "Otwórz za pomocą" window for this file
    OpenWith(PathBuf),
    /// the cloud's own sharing link to the clipboard (OneDrive, Google Drive)
    CloudLink(PathBuf),
    /// the selection always on this device (true), or only in the cloud again
    CloudKeep(bool),
    /// the preview's "Pobierz, aby zobaczyć podgląd": this cloud file's content to this device
    CloudFetch(PathBuf),
    /// Windows' own menu for the selection, from this row's menu (at the app menu's place when it
    /// was opened from the keyboard, else at the pointer) or Shift+right click on it
    ShellMenu(PathBuf, Option<egui::Pos2>),
}

use rx_core::undo::Undo;

struct Rename {
    path: PathBuf,
    text: String,
    fresh: bool,
}

struct App {
    tabs: Vec<Tab>,
    cur: usize,
    /// the search box edits the current tab's name filter instead of the search
    filter_open: bool,
    /// the index, only while "szukaj wszędzie" is in use (freed a few minutes after, see listing.rs)
    index: Option<Arc<rx_core::index::Index>>,
    /// the index is being read or rescanned in the background
    index_busy: bool,
    /// when this session last finished a rescan, and when the index was last searched
    index_scanned: Option<Instant>,
    index_used: Instant,
    /// counts the indexes received, so results made from an older one are redone
    index_gen: u64,
    /// "Analiza miejsca" (as WizTree): folders show their whole size, lists sort biggest first
    space: bool,
    /// bytes under every index entry and every folder's entries, for the index of that generation
    space_data: Option<(u64, SpaceData)>,
    /// bumped when a deletion is taken off space_data, so the treemap is laid out again
    space_ver: u64,
    /// the treemap as last laid out, and what its context menu was opened on: the file or folder
    /// under the pointer and the folders above it up to the folder shown (paths, not index ids:
    /// a rescan may swap the index while the menu is open)
    tmap: Option<TreeMap>,
    tmap_menu: Vec<(PathBuf, bool)>,
    /// paths deleted in this mode: taken off every newer index too while they stay gone (a scan
    /// that started before the deletion may still hold them)
    forgotten: Vec<PathBuf>,
    /// a test run (--biggest): list the biggest files once the index is in
    shot_biggest: bool,
    /// a file operation in this mode: the index is rescanned, so folder sizes catch up
    space_stale: bool,
    /// the list's focus moved by keyboard, so its focus ring shows (Explorer hides it for the mouse)
    kbd_focus: bool,
    /// type-ahead: letters typed in the list and when the last one came
    typed: String,
    typed_at: f64,
    /// the notification "still running in the tray" was shown (once ever, kept in the layout)
    told_tray: bool,
    /// split view: the pane being drawn is the inactive one (its selection is grey)
    inactive_pane: bool,
    /// the folder the tree last showed as current (its row is scrolled into view on arrival)
    tree_at: PathBuf,
    /// Widok ▸ Pełne drzewo folderów: folders on the way to the current one show all their folders
    tree_full: bool,
    /// the tree row the arrow keys move (index in tree_rows and its id); None = keys go to the list
    tree_focus: Option<(usize, PathBuf)>,
    /// the tree's visible rows in the last frame: id, target, expandable, depth
    tree_rows: Vec<(PathBuf, Option<PathBuf>, bool, usize)>,
    /// the focused tree row must scroll into view
    tree_scroll: bool,
    /// Shift+F10 / Menu key: the item (or, for the list's own menu, the folder) whose context
    /// menu opens this frame
    want_menu: Option<PathBuf>,
    /// a context menu opened from the keyboard and where it shows (there is no pointer position)
    menu_at: Option<(egui::Id, egui::Pos2)>,
    /// Plik, Edycja, Widok, Narzędzia buttons, so F10 and Alt+letter open them
    menus: [egui::Id; 4],
    about: bool,
    /// the password dialog of an encrypted archive: archive, typed password, the last one was
    /// wrong, unpack it here
    ask_pw: Option<(PathBuf, String, bool, bool)>,
    /// VirusTotal: each file's check (with the file's modified time it is about), and the key being
    /// typed with the file to check after it and whether a key is kept already
    vt: HashMap<PathBuf, (Option<SystemTime>, Vt)>,
    ask_vt_key: Option<(String, Option<PathBuf>, bool)>,
    /// further encrypted archives (whether their last password was wrong, unpack here), asked one at a time
    pw_queue: std::collections::VecDeque<(PathBuf, bool, bool)>,
    /// the Spakuj dialog; `pack_kind` and `pack_opts` (no password kept) are what was picked last
    ask_pack: Option<archive::PackAsk>,
    /// a Tak/Anuluj question (deleting from an archive, writing an edited copy back, RAR to ZIP)
    ask: Option<archive::Ask>,
    /// files opened from archives, watched for saves (archive.rs)
    arc_watch: Vec<archive::Watched>,
    pack_kind: &'static str,
    pack_opts: rx_archive::PackOpts,
    show_hidden: bool,
    /// Widok ▸ Motyw, as given to egui (kept here for save())
    theme: egui::ThemePreference,
    lock_format: bool,
    /// sorting set by hand per folder (lowercase path), applied whenever the folder is opened
    folder_sorts: HashMap<String, (Col, bool)>,
    view: ViewMode,
    show_preview: bool,
    /// the preview card's width in screen pixels at the last frame, so a PDF page renders as sharp as shown
    preview_px: u32,
    /// the width the shown PDF page was asked for, to tell when the pane has outgrown it
    preview_req_px: u32,
    /// height of the details and buttons under the preview card at the last frame, kept clear of it
    preview_details_h: f32,
    /// the previewed picture's zoom (1 = fit the card) and the file it is for, so a new file starts fitted
    preview_zoom: (PathBuf, f32),
    /// drag and drop: a folder row of the pane being drawn takes the drop (not the pane itself)
    drop_on_row: bool,
    /// drag and drop: where a drop would go at this frame, and whether it would move, for the hint
    drop_target: Option<(PathBuf, bool)>,
    /// a dragged tab or address-bar folder is over a place that takes it: what letting go there does, for the card
    tab_target: Option<&'static str>,
    /// split view: the other pane shows tabs[other]
    split: bool,
    other: usize,
    /// drives for the "Ten komputer" view, and whether they were asked for since they last changed
    pc: Option<Vec<PcRow>>,
    pc_asked: bool,
    widths: [f32; 4],
    /// width of the Nazwa column set by dragging its border; 0 = it fills the rest
    name_width: f32,
    names: HashMap<PathBuf, String>,
    tree_open: HashSet<PathBuf>,
    /// the folders in each folder the tree showed, as last read, and the folders asked about
    /// since the last change on disk (the others are read again when drawn)
    tree_kids: HashMap<PathBuf, Vec<PathBuf>>,
    tree_asked: HashMap<PathBuf, rx_core::Asked<()>>,
    /// folders whose size is being counted in the background: "…" in Wielkość until it comes
    sizing: HashSet<PathBuf>,
    /// hidden or system folders the tree lists while hidden files are shown (Ctrl+H): drawn dimmed
    tree_dim: HashSet<PathBuf>,
    bookmarks: Vec<PathBuf>,
    /// root, name and whether it is a network drive; under its own name ("C:\") until the name
    /// Explorer shows is read, which `drives_unread` tells per letter (bits as sys::drive_mask)
    drives: Vec<(PathBuf, String, bool)>,
    drives_unread: u32,
    known: Vec<(PathBuf, String, Kind)>,
    /// `known` and `clouds` were read (the first moments of a run)
    places_read: bool,
    /// cloud folders whose files get a mark of where they are (in the cloud or on this device)
    clouds: Vec<PathBuf>,
    clip: Vec<PathBuf>,
    cut: bool,
    /// the Windows clipboard's number when it was last looked at; a new content there makes `clip` obsolete
    clip_seq: u32,
    /// `clip` came from or is also on the Windows clipboard (not items of an archive)
    clip_sys: bool,
    /// the Windows clipboard holds files (Wklej reads them only when used)
    clip_files: bool,
    /// the Windows clipboard's number of a cut whose paste here runs while its job holds the Arc
    /// (a second paste of it meanwhile is refused)
    pasting: Option<(u32, Arc<()>)>,
    /// files dragged in from another program: where the pointer is and the files; true at the drop
    ext: Option<(egui::Pos2, Arc<Dragged>, bool)>,
    /// a system drag (dragged out of the window) has ended: egui never saw the button let go
    drag_out_done: bool,
    /// a drag left the window: the system drag starts next frame, once this one is drawn without the card
    drag_out_next: Option<Arc<Dragged>>,
    /// what the last system drag carried: dropped back here, it keeps its pane
    dragged_out: Option<Arc<Dragged>>,
    /// Windows' menu, ready: shown next frame, once this one is drawn without the app's own menu
    /// and with any note about it (the menu's loop blocks drawing until it closes)
    shell_menu_next: Option<(sys::menu::Items, Option<(i32, i32)>)>,
    undo: rx_core::undo::Stack,
    rename: Option<Rename>,
    addr_edit: Option<String>,
    msg: Option<String>,
    /// the note shown and since when (src/bars.rs: it goes after a while)
    msg_at: Option<(String, f64)>,
    pending: Vec<Act>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// the thumbnail thread's requests, in the order asked; poll drops those scrolled away
    thumb_req: Arc<rx_core::Requests<PathBuf>>,
    thumbs: HashMap<PathBuf, Thumb>,
    preview: Option<(PathBuf, Shown)>,
    /// the preview thread's next request: the file, the card's width in pixels and whether to read
    /// a file whose attributes say it is only in the cloud (see build_preview)
    preview_req: Arc<rx_core::Latest<(PathBuf, u32, bool)>>,
    /// free and total bytes for the status bar as last answered, and the folder they are asked
    /// for now (None: to be asked again)
    disk: Option<(PathBuf, u64, u64)>,
    disk_asked: Option<PathBuf>,
    title: String,
    /// when the files opened from archives were last looked at (check_watched)
    last_check: Instant,
    /// reports changes other programs make to the folders on screen, and what it was last given:
    /// each shown tab's path with its folder on disk (the archive's folder for a tab in one)
    watcher: sys::watch::Watcher,
    watched: Vec<(PathBuf, PathBuf)>,
    /// since when the layout may differ from the one stored: the first key press or pointer move
    /// after the last save (see `save_soon`)
    unsaved: Option<Instant>,
    shot: Option<PathBuf>,
    frames: u32,
    /// screenshot mode: seconds from the program's start to its first frame, and to the first
    /// frame with every list in beside egui's time then, from which --shot-after counts
    shot_first: Option<f64>,
    shot_from: Option<(f64, f64)>,
    shot_done: Arc<AtomicBool>,
    /// screenshot mode: ms after start to take the shot; None once it is asked for
    shot_after: Option<f64>,
    /// screenshot mode (--idle): no repaint is forced between frame 120 and the shot
    shot_idle: bool,
    /// screenshot mode (--scroll-end): the item the list has been scrolled to; None once at the end
    shot_scroll: Option<usize>,
    /// screenshot mode (--click-type <text>): Ctrl+L, a click in the search box, then this text, as
    /// egui events (no real input), to see the box take focus from the address bar
    shot_type: Option<String>,
    zoom: f32,
    logo: TextureHandle,
    /// file operations (copy, move, delete…) still running; an update waits for them
    busy: Arc<AtomicUsize>,
    /// the running and just finished operations, for the status bar and the Operacje panel
    ops: Arc<rx_core::ops::Ops>,
    update_rx: Option<Receiver<PathBuf>>,
    update_staged: Option<PathBuf>,
    /// seconds since start of the last key press or pointer move
    last_input: f64,
    started: bool,
    /// kept alive for the icon to stay next to the clock
    _tray: Option<tray_icon::TrayIcon>,
    tray_rx: Receiver<TrayMsg>,
    /// the window really closes (otherwise closing only minimizes it)
    quitting: bool,
    /// the window is hidden in the tray (no taskbar button): counts as minimized for updates and trimming
    hidden: bool,
    /// the window was asked for while minimized: focus it once it is restored
    focus_pending: bool,
    /// the window's HWND, 0 until known
    hwnd: isize,
    /// pixels per point (DPI × zoom) the minimum size was last given to Windows at
    min_ppp: f32,
    /// the window's place, size and maximized state as eframe last stored them while it was on
    /// screen (see `save`)
    last_window: Option<String>,
    /// when the window was minimized, and whether its memory was trimmed since
    minimized_since: Option<Instant>,
    trimmed: bool,
    /// Windows 11's efficiency mode is on for the process (only ever with the window out of sight)
    eco: bool,
    /// started out of sight by an update while maximized: maximized again once on screen
    restore_maximized: bool,
}

fn bookmarks_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(APP_NAME).join("zakladki.txt"))
}

/// One-time move of bookmarks and saved tabs from the app's old name, so the rename loses nothing.
fn migrate_old_name() {
    let moves = [
        (dirs::config_dir().map(|d| d.join("MenedzerPlikow")), dirs::config_dir().map(|d| d.join(APP_NAME))),
        (eframe::storage_dir("Menedżer plików"), eframe::storage_dir(APP_NAME)),
    ];
    for (old, new) in moves {
        if let (Some(old), Some(new)) = (old, new) {
            if old.exists() && !new.exists() {
                if let Some(parent) = new.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::rename(&old, &new);
            }
        }
    }
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        // eframe has just stored the window as Windows reports it; minimized or in the tray that is
        // a stub size at -32000 and "not maximized", so the next start (an update restarts the app
        // in the background) opened a tiny window. The last state from the screen is kept instead.
        // Windows is asked, not egui: a window started hidden by an update (sys::start_minimized)
        // is minimized behind winit's back, so egui's "minimized" flag stays false.
        // shortcut: eframe stores the window every 30 s, so a resize in the last 30 s before the
        // window left the screen is lost; store it on minimize/hide if that turns out to matter
        // Never a place at -32000 (where Windows parks a minimized window): eframe keeps the place
        // a maximized window restores to from a save taken while minimized, and such a store
        // brought the window back off screen after an update. With nothing better, nothing is
        // stored, so the next start opens at the start size.
        // shortcut: while such a window stays maximized eframe offers -32000 at every save, so a
        // move of it in that time is not kept; rebuild the stored settings if that matters
        let now = storage.get_string(WINDOW_KEY).filter(|w| !w.contains("-32000"));
        if self.hwnd != 0 && now.is_some() && sys::window_on_screen(self.hwnd) {
            self.last_window = now;
        } else if let Some(w) = &self.last_window {
            storage.set_string(WINDOW_KEY, w.clone());
        } else if now.is_none() {
            storage.set_string(WINDOW_KEY, String::new());
        }
        if self.shot.is_some() {
            return;
        }
        self.unsaved = None;
        let tabs: Vec<String> = self.tabs.iter().map(|t| t.path.display().to_string()).collect();
        let w = self.widths;
        let view = match self.view {
            ViewMode::Details => 0,
            ViewMode::Thumbs => 1,
            ViewMode::DetailsThumbs => 2,
        };
        let theme = match self.theme {
            egui::ThemePreference::System => 0,
            egui::ThemePreference::Light => 1,
            egui::ThemePreference::Dark => 2,
        };
        let text = format!(
            "zoom={}\nwidths={},{},{},{}\nnamew={}\nview={view}\ntheme={theme}\npreview={}\nhidden={}\ntraynote={}\nfulltree={}\nsplit={}\ncur={}\nother={}\ntabs={}\nsides={}\nlang={}",
            self.zoom, w[0], w[1], w[2], w[3], self.name_width, self.show_preview as u8, self.show_hidden as u8, self.told_tray as u8, self.tree_full as u8, self.split as u8, self.cur, self.other, tabs.join("|"),
            self.tabs.iter().map(|t| t.side.to_string()).collect::<Vec<_>>().join(","),
            rx_core::i18n::lang().code()
        );
        storage.set_string("uklad", text);
        storage.set_string("sortowanie", sorts_to_text(&self.folder_sorts));
        storage.set_string("virustotal", vt_to_text(&self.vt));
    }

    /// Split view: Tab moves to the other pane, as in two-pane file managers. Taken out before
    /// egui sees it, or it would also park focus on the first button and the arrows would stop
    /// moving the selection.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // a system drag out of the window took the button's release: egui would think it still held
        if std::mem::take(&mut self.drag_out_done) {
            let (pos, modifiers) = ctx.input(|i| (i.pointer.latest_pos().unwrap_or_default(), i.modifiers));
            raw.events.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers });
        }
        // egui learns of window focus only from winit's Focused event; with it missed (a window brought
        // to the front from the tray or after an update) no text field showed its caret or took a
        // click's focus until a switch to another window and back. Windows has the truth.
        if self.shot.is_none() && self.hwnd != 0 {
            let fg = sys::is_foreground(self.hwnd);
            if fg && !raw.focused {
                static TOLD: std::sync::Once = std::sync::Once::new();
                TOLD.call_once(|| update::log("okno: na pierwszym planie, a egui uważało je za nieaktywne"));
            }
            raw.focused = fg;
        }
        clipboard_keys(ctx, raw);
        if let Some(text) = &self.shot_type {
            let at = ctx.read_response(egui::Id::unique("search")).map(|r| r.rect.center());
            let press = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            match (self.frames, at) {
                // the address bar in edit first (Ctrl+L): the click must take focus from it
                (50, _) => raw.events.push(egui::Event::Key { key: Key::L, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::CTRL | Modifiers::COMMAND }),
                (60, Some(p)) => raw.events.extend([egui::Event::PointerMoved(p), press(p, true)]),
                (61, Some(p)) => raw.events.push(press(p, false)),
                (70, _) => raw.events.push(egui::Event::Text(text.clone())),
                _ => {}
            }
        }
        if !self.split || self.about || ctx.text_edit_focused() || ctx.memory(|m| m.focused().is_some()) || crate::ops::popup_blocks(ctx) {
            return;
        }
        let before = raw.events.len();
        raw.events.retain(|e| !matches!(e, egui::Event::Key { key: Key::Tab, pressed: true, modifiers, .. } if !modifiers.ctrl && !modifiers.alt));
        if raw.events.len() < before {
            self.pending.push(Act::SwitchPane);
        }
    }

    /// Runs before every ui() and also while the window is minimized or hidden, when eframe skips
    /// ui() (and egui's input time stands still): the tray, updates and memory trimming live here.
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        follow_theme(ctx);
        // here too: the first lists are read while the first frame is drawn, and a window started
        // in the tray has them when it is shown
        self.send_listings(ctx);
        self.watch_shown(ctx);
        if self.shot.is_none() {
            if !self.started {
                // the window is up (also minimized or in the tray, when ui() does not run): a
                // freshly installed build tells its guard, then updates are looked for
                self.started = true;
                update::startup_complete();
                self.update_rx = update::start(wake);
            }
            self.window_handle(frame);
            if self.hwnd != 0 && sys::bring_onto_screen(self.hwnd) {
                update::log("okno: poza ekranem, przeniesione na ekran główny");
            }
            let on_screen = self.hwnd != 0 && sys::window_on_screen(self.hwnd);
            // until egui sees it maximized: the size check below would undo a maximize still on its way
            let maximize_now = self.restore_maximized && on_screen;
            if maximize_now {
                if ctx.input(|i| i.viewport().maximized) == Some(true) {
                    self.restore_maximized = false;
                } else {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
                }
            }
            // eframe hands Windows the minimum once, in pixels at the start's DPI and zoom: on a
            // screen with a larger scale Windows let an edge drag below the check below, which
            // then jumped to the start size against the drag (the window flickered while narrowed)
            let ppp = ctx.input(|i| i.viewport().native_pixels_per_point).unwrap_or(1.0) * ctx.zoom_factor();
            if ppp != self.min_ppp {
                self.min_ppp = ppp;
                // in egui points, which egui-winit scales by DPI × zoom
                ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::Vec2::from(MIN_SIZE) / ctx.zoom_factor()));
            }
            // Windows keeps the minimum only while the edges are dragged: a size restored from the
            // saved layout skipped it (a stub 158×64 window came back at every start), so a window
            // on screen below it gets the start size again. Its size is Windows's own: egui's still
            // says 158×26 just after a restore. A maximized window is left alone (a screen smaller
            // than the minimum would otherwise un-maximize it).
            if on_screen && !maximize_now && ctx.input(|i| i.viewport().maximized) != Some(true) {
                let (w, h) = sys::client_size(self.hwnd);
                let px = ctx.input(|i| i.viewport().native_pixels_per_point).unwrap_or(1.0);
                if (w as f32) < MIN_SIZE[0] * px - 1.0 || (h as f32) < MIN_SIZE[1] * px - 1.0 {
                    // once a run: a screen smaller than the minimum would ask again every frame
                    static LOGGED: std::sync::Once = std::sync::Once::new();
                    LOGGED.call_once(|| update::log(format!("okno: {w}×{h} px poniżej minimum, rozmiar startowy")));
                    // in egui points, which the app's own zoom (Ctrl+Plus) scales
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::Vec2::from(START_SIZE) / ctx.zoom_factor()));
                }
            }
            // the first X ever hides to the tray (tray_and_close): a note of the app's own says so
            if !self.told_tray && !self.quitting && self._tray.is_some() && ctx.input(|i| i.viewport().close_requested()) {
                self.told_tray = true;
                sys::note::show(&t!("main.tray_note.title", app = APP_NAME), t!("main.tray_note.text"), 6, dark(), ask_to_show);
            }
            if !self.tray_and_close(ctx) {
                self.install_update(ctx);
            }
            self.trim_when_minimized(ctx);
            self.save_soon(ctx, frame);
            // here, not in ui(): it also runs while the window is minimized or in the tray
            if self.index.is_some() && !self.index_busy {
                const KEEP: std::time::Duration = std::time::Duration::from_secs(300);
                match KEEP.checked_sub(self.index_used.elapsed()) {
                    // nothing else wakes an idle or minimized window to let the index go
                    Some(left) => ctx.request_repaint_after(left + std::time::Duration::from_secs(1)),
                    None => {
                        self.index = None;
                        // what was worked out from it goes with it (~50 MB for 3 million entries)
                        (self.space_data, self.tmap) = (None, None);
                    }
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.zoom = ctx.zoom_factor();
        self.poll(&ctx);
        self.clipboard_and_drops(&ctx);
        self.modal_loops(&ctx);
        self.keys(&ctx);

        let bar = |m: Margin| egui::Frame::new().fill(pal().bar).inner_margin(m);
        // three surface levels (bar: title and status, pane: working layer, content) told
        // apart by hairlines in pal().line, so the window does not run together into one grey block
        egui::Panel::top("menu").frame(bar(Margin { left: 6, right: 0, top: 0, bottom: 0 })).show_separator_line(true).show(ui, |ui| {
            // empty parts of the menu bar act as the title bar: drag to move, double-click to maximise
            let title = ui.interact(ui.max_rect(), ui.scope_id().with("titlebar"), Sense::click_and_drag());
            if title.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if title.double_clicked() {
                let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
            }
            self.menu_bar(ui);
        });
        egui::Panel::top("nav").frame(egui::Frame::new().fill(pal().pane).inner_margin(Margin::symmetric(8, 6))).show_separator_line(true).show(ui, |ui| self.nav_bar(ui));
        egui::Panel::bottom("status").frame(bar(Margin::symmetric(12, 4))).show_separator_line(true).show(ui, |ui| self.status_bar(ui));
        // a narrow window (a snap zone) gives its width to the list: the side panes are only left
        // out, so they come back as set once the window widens
        let wide = ui.available_width();
        if wide >= NARROW[0] {
            egui::Panel::left("tree")
                .frame(egui::Frame::new().fill(pal().pane).inner_margin(Margin { left: 4, right: 2, top: 4, bottom: 2 }))
                .show_separator_line(true)
                .resizable(true)
                .default_size(270.0)
                .size_range(160.0..=500.0)
                .show(ui, |ui| self.tree(ui));
        } else {
            // (its keys would otherwise go on moving through rows nobody sees)
            self.tree_focus = None;
        }
        if self.show_preview && wide >= NARROW[1] {
            egui::Panel::right("preview")
                .frame(egui::Frame::new().fill(pal().pane).inner_margin(Margin::same(8)))
                .show_separator_line(true)
                .resizable(true)
                .default_size(300.0)
                .size_range(220.0..=900.0)
                .show(ui, |ui| self.preview_ui(ui));
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal().content)).show(ui, |ui| {
            if !self.split {
                egui::Panel::top("tabs").frame(egui::Frame::new().fill(pal().pane).inner_margin(Margin { left: 8, right: 8, top: 4, bottom: 0 })).show_separator_line(false).show(ui, |ui| {
                    let (c, plus) = self.tabs_bar(ui, None, self.cur);
                    if let Some(c) = c {
                        self.cur = c;
                    }
                    if plus {
                        self.pending.push(Act::NewTab(None));
                    }
                });
            }
            // "Analiza miejsca": the treemap of the current folder under the list
            if self.space {
                egui::Panel::bottom("treemap")
                    .frame(egui::Frame::new().fill(pal().pane).inner_margin(Margin { left: 8, right: 8, top: 6, bottom: 8 }))
                    .show_separator_line(true)
                    .resizable(true)
                    .default_size(260.0)
                    .size_range(120.0..=800.0)
                    .show(ui, |ui| self.treemap_ui(ui));
            }
            // 6px on the right keeps the scrollbar clear of the window's resize edge
            egui::CentralPanel::default().frame(egui::Frame::new().fill(pal().content).inner_margin(Margin { left: 0, right: 6, top: 0, bottom: 0 })).show(ui, |ui| {
                if self.split {
                    self.split_panes(ui);
                } else {
                    let r = ui.max_rect();
                    self.file_list(ui);
                    // a tab dropped on the right half opens the split view with it there
                    self.tab_drop_zone(ui, Rect::from_min_max(pos2(r.center().x, r.top()), r.max), 1);
                }
            });
        });

        // a Shift+F10 menu opens in the frame it was asked for, or not at all
        self.want_menu = None;
        self.vt_key_dialog(&ctx);
        // drag and drop: the card under the pointer says what is held and what letting go would do
        let target = self.drop_target.take();
        if self.ext.as_ref().is_some_and(|x| x.2) {
            self.ext = None;
            self.msg = Some(t!("main.drop.refused").into());
        }
        let drag = egui::DragAndDrop::payload::<Dragged>(&ctx);
        if let Some(d) = drag.filter(|_| self.drag_out_next.is_none() && !self.drag_out_done) {
            let name = d.0.first().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let name = if d.0.len() > 1 { t!("main.drag.and_more", name = name, n = d.0.len() - 1) } else { name };
            let action = match &target {
                Some((to, mv)) => {
                    ctx.set_cursor_icon(if *mv { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Copy });
                    let to = to.file_name().map_or_else(|| to.to_string_lossy(), |s| s.to_string_lossy());
                    if *mv { t!("main.drag.move_to", to = to) } else { t!("main.drag.copy_to", to = to) }
                }
                None => t!("main.drag.hint").into(),
            };
            drag_card(&ctx, d.2, &name, d.0.len(), &action, target.is_some());
        }
        let tab_target = self.tab_target.take();
        if let Some(t) = egui::DragAndDrop::payload::<DraggedTab>(&ctx).filter(|t| t.0 < self.tabs.len()) {
            let hint = if self.split { t!("main.drag.other_pane") } else { t!("main.drag.split") };
            let kind = if is_pc(&self.tabs[t.0].path) { Kind::Computer } else { Kind::Folder };
            drag_card(&ctx, kind, &self.tab_label(t.0), 1, tab_target.unwrap_or(hint), tab_target.is_some());
        }
        if let Some(d) = egui::DragAndDrop::payload::<DraggedPath>(&ctx) {
            drag_card(&ctx, Kind::Folder, &self.dname(&d.0), 1, tab_target.unwrap_or(t!("main.drag.bookmarks")), tab_target.is_some());
        }
        if self.about {
            let (version, build) = update::version();
            let mut close = false;
            let r = egui::Modal::new(egui::Id::unique("o_programie")).show(&ctx, |ui| {
                ui.set_width(360.0);
                ui.horizontal(|ui| {
                    ui.add(egui::Image::new((self.logo.id(), vec2(32.0, 32.0))));
                    ui.label(RichText::new(APP_NAME).heading());
                });
                ui.add_space(8.0);
                ui.label(t!("main.about.version", version = version, build = build));
                // the version rises after every update
                ui.label(RichText::new(t!("main.about.version_note")).color(pal().dim));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(t!("main.about.author"));
                    ui.hyperlink_to("github.com/jarx-dev", "https://github.com/jarx-dev");
                });
                ui.horizontal(|ui| {
                    ui.label(t!("main.about.source"));
                    ui.hyperlink_to("github.com/jarx-dev/RustExplorer", "https://github.com/jarx-dev/RustExplorer");
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new(t!("main.about.license")).color(pal().dim));
                    ui.hyperlink_to(t!("main.about.coffee"), "https://buymeacoffee.com/jarx");
                });
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    close = ui.add(egui::Button::new(t!("common.close")).min_size(vec2(96.0, 32.0)).fill(pal().field).stroke(Stroke::new(1.0, pal().line))).clicked();
                });
            });
            if close || r.should_close() {
                self.about = false;
            }
        }
        // an encrypted archive asks for its password; Enter unpacks, Esc or Anuluj gives up
        if let Some((arc, pw, wrong, here)) = &mut self.ask_pw {
            let (mut go, mut cancel) = (false, false);
            let r = egui::Modal::new(egui::Id::unique("haslo")).show(&ctx, |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new(t!("main.pw.title")).heading());
                ui.add_space(6.0);
                ui.label(arc.file_name().unwrap_or_default().to_string_lossy());
                ui.add_space(8.0);
                let e = ui.add(egui::TextEdit::singleline(pw).password(true).hint_text(t!("main.pw.hint")).desired_width(f32::INFINITY));
                if !e.has_focus() && !e.lost_focus() {
                    e.request_focus();
                }
                go = e.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if *wrong {
                    ui.label(RichText::new(t!("main.pw.wrong")).color(pal().red));
                }
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    let plain = |t| egui::Button::new(t).min_size(vec2(96.0, 32.0)).fill(pal().field).stroke(Stroke::new(1.0, pal().line));
                    cancel = ui.add(plain(t!("common.cancel"))).clicked();
                    go |= ui.add(plain(t!("main.pw.extract"))).clicked();
                });
            });
            if go && !pw.is_empty() {
                let (a, p, h) = (arc.clone(), std::mem::take(pw), *here);
                self.ask_pw = self.pw_queue.pop_front().map(|(a, w, h)| (a, String::new(), w, h));
                self.start_extract(a, Some(p), &[], h, &ctx);
            } else if cancel || r.should_close() {
                self.ask_pw = self.pw_queue.pop_front().map(|(a, w, h)| (a, String::new(), w, h));
            }
        }
        self.pack_dialog(&ctx);
        // a Tak/Anuluj question; Enter says yes, Esc or Anuluj no
        if let Some(q) = &self.ask {
            let (mut yes, mut no) = (false, false);
            let r = egui::Modal::new(egui::Id::unique("pytanie")).show(&ctx, |ui| {
                ui.set_width(400.0);
                ui.label(RichText::new(&q.title).heading());
                ui.add_space(8.0);
                ui.label(&q.text);
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    let plain = |t: &str| egui::Button::new(t).min_size(vec2(96.0, 32.0)).fill(pal().field).stroke(Stroke::new(1.0, pal().line));
                    no = ui.add(plain(t!("common.cancel"))).clicked();
                    let b = ui.add(plain(&q.yes));
                    if !b.has_focus() {
                        b.request_focus();
                    }
                    yes = b.clicked() || ui.input(|i| i.key_pressed(Key::Enter));
                });
            });
            if yes {
                let q = self.ask.take().unwrap();
                self.pending.push(q.act);
            } else if no || r.should_close() {
                self.ask = None;
            }
        }
        resize_edges(&ctx);
        // a visible ring on the widget reached with Tab (only TextEdit takes focus on click)
        if !ctx.text_edit_focused() {
            if let Some(r) = ctx.memory(|m| m.focused()).and_then(|id| ctx.read_response(id)) {
                let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("focus")));
                p.rect_stroke(r.rect.expand(1.0), cr(5.0), Stroke::new(2.0, pal().text), StrokeKind::Outside);
            }
        }
        for a in std::mem::take(&mut self.pending) {
            // an earlier command may have changed the rows (Analiza miejsca on C:\ went to the
            // drive, then the next one read the old view past the new rows and crashed)
            self.refresh_views();
            self.run(a, &ctx);
        }
        self.send_listings(&ctx);

        if let Some(shot) = &self.shot {
            // lists and drives are read on other threads: the frames and --shot-after count from
            // the first frame that has them all, or a shot could show a list before its rows
            let now = ctx.input(|i| i.time);
            let run = STARTED.get().map_or(0.0, |t| t.elapsed().as_secs_f64());
            self.shot_first.get_or_insert(run);
            let pc_waits = is_pc(&self.tabs[self.cur].path) && !self.pc.as_ref().is_some_and(|p| p.len() == self.drives.len());
            let waits = !self.places_read || self.drives_unread != 0 || pc_waits || self.tabs.iter().any(|t| t.loading()) || self.tree_asked.values().any(|a| a.waiting());
            if self.shot_from.is_none() && !waits {
                self.shot_from = Some((run, now));
            }
            let Some((listed, from)) = self.shot_from else {
                ctx.request_repaint();
                return;
            };
            let now = now - from;
            self.frames += 1;
            // --scroll-end: 8 items a frame, so every row is drawn (and asks for its thumbnail) once
            if let Some(i) = self.shot_scroll.filter(|_| self.frames > 120) {
                let t = &mut self.tabs[self.cur];
                t.scroll_to = Some((i + 8).min(t.view.len().saturating_sub(1)));
                self.shot_scroll = (i + 8 < t.view.len()).then_some(i + 8);
            }
            // --idle: once loaded and scrolled the run sleeps until the shot is due, as a window left
            // alone does, so the CPU it uses meanwhile is the app's own
            match self.shot_after.map(|ms| ms / 1000.0 - now) {
                Some(s) if s > 0.0 && self.shot_idle && self.frames >= 120 && self.shot_scroll.is_none() => ctx.request_repaint_after(std::time::Duration::from_secs_f64(s)),
                _ => ctx.request_repaint(),
            }
            // the shot waits 120 frames and --shot-after <ms> (lets a Windows test measure memory over time)
            if self.frames >= 120 && self.shot_after.is_some_and(|ms| now * 1000.0 >= ms) {
                self.shot_after = None;
                // --idle: the frames drawn by now (120 to load, the rest are the app's own wake-ups) go
                // next to the shot; process CPU time is too coarse to show a few short wake-ups.
                // Under them: how long after its start the program drew its first frame, and the lists
                if self.shot_idle {
                    let first = self.shot_first.unwrap_or(0.0) * 1000.0;
                    let _ = std::fs::write(shot.with_extension("frames.txt"), format!("{}\npierwsza klatka po {first:.0} ms, z listami po {:.0} ms", self.frames, listed * 1000.0));
                }
                self.frames = 0;
                let (shot, done) = (shot.clone(), self.shot_done.clone());
                ctx.request_screenshot(move |img| {
                    let [w, h] = img.size;
                    let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                    if let Some(parent) = shot.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Err(e) = image::save_buffer(&shot, &bytes, w as u32, h as u32, image::ColorType::Rgba8) {
                        eprintln!("zrzut ekranu: {e}");
                    }
                    done.store(true, AO::SeqCst);
                });
            }
            if self.shot_done.load(AO::SeqCst) || (self.shot_after.is_none() && self.frames > 600) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}

/// The clipboard keys, which egui-winit no longer turns into events (clipboard_shortcuts off): in a
/// text field (or a selected label, for Copy) they are its text events; elsewhere Ctrl+C/X/V keys
/// for the list's key table, Ctrl+Insert and Shift+Insert as well; Shift+Delete stays the Delete key.
fn clipboard_keys(ctx: &egui::Context, raw: &mut egui::RawInput) {
    let text = ctx.text_edit_focused();
    let labels = ctx.plugin_opt::<egui::text_selection::LabelSelectionState>();
    let mut label = labels.as_ref().is_some_and(|p| p.lock().has_selection());
    for e in std::mem::take(&mut raw.events) {
        let what = match &e {
            egui::Event::Key { key, pressed: true, modifiers, .. } => clip_key(*modifiers, *key),
            _ => None,
        };
        // egui keeps a label's (even empty) selection after any click on it until a click elsewhere;
        // another key is work in the list, after which Ctrl+C copies the files again
        if label && what.is_none() && matches!(&e, egui::Event::Key { pressed: true, .. }) {
            labels.iter().for_each(|p| p.lock().clear_selection());
            label = false;
        }
        let new = match what {
            None => Some(e),
            Some(Key::C) if text || label => Some(egui::Event::Copy),
            Some(Key::X) if text => Some(egui::Event::Cut),
            // read only on the key press, as egui-winit did
            Some(_) if text => sys::clip::text().map(|t| t.replace("\r\n", "\n")).filter(|t| !t.is_empty()).map(egui::Event::Paste),
            // Shift+Delete stays a delete (to the Recycle Bin, undoable), not a cut of the selection
            Some(_) if matches!(&e, egui::Event::Key { key: Key::Delete, .. }) => Some(e),
            Some(key) => Some(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::CTRL | Modifiers::COMMAND }),
        };
        raw.events.extend(new);
    }
}

/// The DirectX 12 fallback, used only when OpenGL does not start: wgpu's default "Performance" hints
/// reserve 128-256 MB GPU blocks, which on an integrated GPU are plain RAM (measured: 455 MB vs 280 MB).
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{wgpu, WgpuSetup};
    let mut o = eframe::egui_wgpu::WgpuConfiguration::default();
    if let WgpuSetup::CreateNew(c) = &mut o.wgpu_setup {
        let base = c.device_descriptor.clone();
        c.device_descriptor = Arc::new(move |a| wgpu::DeviceDescriptor { memory_hints: wgpu::MemoryHints::MemoryUsage, ..base(a) });
    }
    o
}

fn main() -> eframe::Result {
    let _ = STARTED.set(Instant::now());
    let args: Vec<String> = std::env::args().collect();
    // the previous build watching over a freshly installed one (see update.rs)
    if update::run_guard(&args) {
        return Ok(());
    }
    update::finish_pending_update(&args);
    // no console: a panic goes to update.log as well
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        update::log(format!("app: panic: {info}"));
        default_hook(info);
    }));
    let shot = args.iter().position(|a| a == "--screenshot").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let select = args.iter().position(|a| a == "--select").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    // a folder given on the command line opens there instead of the saved tabs. Taken by its
    // look (a full path that is no flag's value), not asked of the disk: a share that does not
    // answer would hold the window back for seconds; a path that is no folder says so in its list
    let valued = ["--screenshot", "--select", "--save-icon", "--find", "--search", "--filter", "--click-type", "--theme", "--size", "--lang"];
    let given = (1..args.len()).filter(|&i| !valued.contains(&args[i - 1].as_str())).map(|i| PathBuf::from(&args[i])).find(|p| p.is_absolute() || is_pc(p));
    let explicit = given.is_some() || select.is_some();
    // none: Documents, asked for only once no saved tab has taken its place (see below)
    let start = given.or_else(|| select.as_ref().and_then(|s| s.parent()).map(Path::to_path_buf));
    // --save-icon <file.png|file.ico>: write the app icon and exit
    if let Some(out) = args.iter().position(|a| a == "--save-icon").and_then(|i| args.get(i + 1)) {
        let mut png = vec![];
        let img = image::RgbaImage::from_raw(256, 256, icon::rgba(256)).expect("icon size");
        image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).expect("png");
        let bytes = if out.to_lowercase().ends_with(".ico") { icon::ico(&png) } else { png };
        std::fs::write(out, bytes).expect("zapis ikony");
        return Ok(());
    }
    if shot.is_none() {
        // starting the app again brings back the running one, also from the tray (an updated
        // build gets here only after finish_pending_update has seen its predecessor exit).
        // shortcut: a folder given to the second start is dropped, pass it on if the app becomes a folder handler
        let woke = sys::single_instance(ask_to_show);
        if !woke {
            // a freshly installed build beaten to it by a start made during the swap: the copy
            // running is this same new EXE, so the update counts as started (no rollback)
            update::startup_complete();
            return Ok(());
        }
        migrate_old_name();
    }
    sys::take_shell_work();
    // the drives and the places of Widok główny are read while the window is being made
    let chan = channel();
    actions::read_drives(chan.0.clone(), true);
    let chan = Arc::new(Mutex::new(Some(chan)));
    // screenshot runs are invisible unless --visible: tests run on a machine that is in use
    let cloaked = shot.is_some() && !args.iter().any(|a| a == "--visible");
    // --size 330x320: a screenshot run's window in points (how a snap zone looks)
    let shot_size = shot.as_ref().and_then(|_| args.iter().position(|a| a == "--size")).and_then(|i| args.get(i + 1)).and_then(|v| v.split_once('x')).and_then(|(w, h)| Some([w.parse().ok()?, h.parse().ok()?]));
    let shot_dir = shot.as_ref().map(|_| std::env::temp_dir().join(format!("rustexplorer-zrzut-{}", std::process::id())));
    let opts = eframe::NativeOptions {
        // no frame, yet eframe keeps the framed window's styles (winit's undecorated shadow), so
        // Windows 11 rounds the corners and draws its shadow and border itself: no DWM call needed
        viewport: egui::ViewportBuilder::default().with_inner_size(shot_size.unwrap_or(START_SIZE)).with_min_inner_size(MIN_SIZE).with_title(APP_NAME).with_decorations(false).with_active(!cloaked).with_taskbar(!cloaked).with_icon(egui::IconData { rgba: icon::rgba(256), width: 256, height: 256 }),
        // screenshot runs use a fresh, throw-away layout so they never touch the person's saved one
        persistence_path: shot_dir.clone(),
        wgpu_options: wgpu_options(),
        // Ctrl+C/X/V reach the list as keys (clipboard_keys): egui-winit turns them into text events,
        // and with only files on the clipboard Ctrl+V into nothing at all
        clipboard_shortcuts: false,
        ..Default::default()
    };
    // OpenGL needs about half the memory of DirectX 12 (measured on an AMD 780M: ~150 vs ~280 MB private);
    // DirectX 12 stays as the fallback for a machine without a usable OpenGL driver
    let launch = |renderer| {
        let (args, start, shot, select, chan) = (args.clone(), start.clone(), shot.clone(), select.clone(), chan.clone());
        eframe::run_native(APP_NAME, eframe::NativeOptions { renderer, ..opts.clone() }, Box::new(move |cc| {
            let is_shot = shot.is_some();
            let _ = APP_CTX.set(cc.egui_ctx.clone());
            let split = args.iter().any(|a| a == "--split");
            // pictures for the README: no network shares of this machine (before App::new lists the drives)
            if is_shot && args.iter().any(|a| a == "--no-network") {
                sys::HIDE_NETWORK_DRIVES.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            let layout = cc.storage.filter(|_| !is_shot).and_then(|st| st.get_string("uklad"));
            // before the window's texts and the tray menu are made; --lang en|pl sets a screenshot
            // run's (its throw-away layout has none, so English)
            let lang = args.iter().position(|a| a == "--lang").and_then(|i| args.get(i + 1)).and_then(|v| rx_core::i18n::Lang::from_code(v));
            rx_core::i18n::set_lang(lang.unwrap_or_else(|| rx_core::i18n::lang_of_layout(layout.as_deref())));
            let mut app = App::new(&cc.egui_ctx, start.unwrap_or_default(), shot, chan.lock().unwrap().take());
            app.last_window = cc.storage.and_then(|s| s.get_string(WINDOW_KEY)).filter(|w| !w.contains("-32000"));
            // restarted by an update while minimized or in the tray: the window goes there before
            // its first frame, with its icons, so nothing flashes (eframe would show it normally first)
            let tray_start = args.iter().any(|a| a == update::TRAY_ARG);
            if !is_shot && (tray_start || args.iter().any(|a| a == update::MINIMIZED_ARG)) {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                if let Ok(RawWindowHandle::Win32(w)) = cc.window_handle().map(|h| h.as_raw()) {
                    // app.hwnd stays 0: the first frame then gives the icons again (window_handle).
                    // The taskbar makes the button right here and asks for the icon while the
                    // message loop is not running yet; unanswered, the button stayed blank
                    let hwnd = w.hwnd.get();
                    sys::set_window_icons(hwnd);
                    app.hidden = tray_start && app._tray.is_some();
                    sys::start_minimized(hwnd, app.hidden);
                    // Windows brings a window minimized before it was ever shown back at its normal
                    // size, not maximized (the stored state then said "not maximized" for good)
                    app.restore_maximized = app.last_window.as_deref().is_some_and(|w| w.replace(' ', "").contains("maximized:true"));
                }
            }
            if cloaked {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                if let Ok(RawWindowHandle::Win32(w)) = cc.window_handle().map(|h| h.as_raw()) {
                    sys::cloak_window(w.hwnd.get());
                }
            }
            if is_shot {
                app.shot_after = Some(args.iter().position(|a| a == "--shot-after").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(0.0));
                // screenshot tests of "szukaj wszędzie" (they cannot type): the query is set up front
                if let Some(q) = args.iter().position(|a| a == "--find").and_then(|i| args.get(i + 1)) {
                    let t = &mut app.tabs[app.cur];
                    (t.search_all, t.search) = (true, q.clone());
                }
                // --search / --filter: the first tab's folder search or name filter (with --split the
                // other pane must stay unfiltered)
                let val = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
                let t = &mut app.tabs[app.cur];
                if let Some(s) = val("--search") {
                    t.search = s;
                }
                if let Some(f) = val("--filter") {
                    t.filter = f;
                }
                // measuring runs: --idle stops the forced repaints between frame 120 and the shot (CPU
                // of an idle window) and writes the frame count to <shot>.frames.txt (under it the
                // time to the first frame and to the first one with every list in), --thumbs starts in
                // Miniatury, --scroll-end scrolls the list through to its last item after frame 120
                // (every thumbnail gets asked for)
                app.shot_idle = args.iter().any(|a| a == "--idle");
                if args.iter().any(|a| a == "--thumbs") {
                    app.view = ViewMode::Thumbs;
                }
                app.shot_scroll = args.iter().any(|a| a == "--scroll-end").then_some(0);
                app.shot_type = val("--click-type");
                // --theme dark|light: the look is fixed (light without it), so a shot never depends
                // on the test machine's Windows setting
                let dark = args.iter().position(|a| a == "--theme").and_then(|i| args.get(i + 1)).is_some_and(|v| v == "dark");
                app.theme = if dark { egui::ThemePreference::Dark } else { egui::ThemePreference::Light };
                cc.egui_ctx.set_theme(app.theme);
            }
            if let Some(text) = layout {
                app.restore(&cc.egui_ctx, &text, explicit);
            }
            // not on every start: the shell checks that Documents is there, slow on a share that does not answer
            if app.tabs[0].path.as_os_str().is_empty() {
                app.tabs[0] = Tab::new(dirs::document_dir().unwrap_or_else(|| PathBuf::from("C:\\")));
            }
            if let Some(text) = cc.storage.filter(|_| !is_shot).and_then(|st| st.get_string("sortowanie")) {
                app.folder_sorts = sorts_from_text(&text);
            }
            if let Some(text) = cc.storage.filter(|_| !is_shot).and_then(|st| st.get_string("virustotal")) {
                app.vt = vt_from_text(&text);
            }
            for i in 0..app.tabs.len() {
                app.apply_folder_sort(i);
            }
            if split {
                app.pending.push(Act::ToggleSplit);
            }
            // test runs: hidden files shown from the start
            if args.iter().any(|a| a == "--hidden") {
                app.show_hidden = true;
            }
            // test runs: "Analiza miejsca" from the start, optionally with the biggest files listed
            app.space = is_shot && args.iter().any(|a| a == "--space");
            app.about = is_shot && args.iter().any(|a| a == "--about");
            // the Spakuj dialog as a 7z with a password typed, for a picture of all its options
            if is_shot && args.iter().any(|a| a == "--pack") {
                app.pack_kind = "7z";
                app.ask_pack = Some(archive::PackAsk { protect: true, pw: "secret".into(), pw2: "secret".into(), ..archive::PackAsk::new("Report".into(), "Folder 1 (+2)".into()) });
            }
            app.shot_biggest = app.space && args.iter().any(|a| a == "--biggest");
            if let Some(s) = select {
                if app.space {
                    // a deeper file opens the tree down to it, as a click on its map tile does
                    app.pending.push(Act::Reveal(s.clone()));
                }
                // selected once the folder's rows are in; the list finds the row by `focus` when it is drawn
                app.tab().asked.then(rx_core::Select { paths: vec![s], rename: false, listed: false });
            }
            // copies opened from archives on earlier days
            std::thread::spawn(archive::clean_temp);
            Ok(Box::new(app))
        }))
    };
    let r = match launch(eframe::Renderer::Glow) {
        Err(e @ (eframe::Error::Glutin(_) | eframe::Error::NoGlutinConfigs(..) | eframe::Error::OpenGL(_))) => {
            eprintln!("OpenGL niedostępny ({e}), uruchamiam DirectX 12");
            launch(eframe::Renderer::Wgpu)
        }
        r => r,
    };
    // the throw-away layout goes with its run, or one folder per test piles up in %TEMP%
    if let Some(d) = &shot_dir {
        let _ = std::fs::remove_dir_all(d);
    }
    // Windows.Data.Pdf keeps a Direct3D device of its own, and the AMD driver fails fast when the
    // process's DLL teardown destroys it; by now the settings are saved and the window and tray
    // icon are gone, so a clean run skips that teardown
    if r.is_ok() {
        sys::end_process(0);
    }
    r
}
