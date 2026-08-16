// SPDX-License-Identifier: BSD-3-Clause

use std::io::{self, Write};

use ratatui::{
    layout::Alignment,
    style::{Color, Modifier, Style},
    text::Line,
};

use crate::themes::Theme;

/*
 * Tips deliberately use a bright terminal green so the final section
 * remains visually discoverable while scrolling rapidly through Help.
 */
const COLOR_TIP_TEXT: Color = Color::Rgb(90, 230, 120);

pub const TIPS_LINK_TEXT: &str = "[Jump to Tips]";

pub const TOP_LINK_TEXT: &str = "[Jump back to top]";

pub fn content(theme: &Theme, text_width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    push_intro_paragraph(
        &mut lines,
        "Scry is a fast terminal file browser for exploring local and remote \
    filesystems. It combines live searching with List and Tree views, detailed \
    metadata inspection, file opening and optional deletion, and SSH browsing \
    backed by persistent remote indexes for fast recursive searches.",
        text_width,
        theme,
    );

    lines.push(Line::raw(""));
    /*
     * In-document jump link.
     *
     * ui.rs replaces this line's style while the pointer is hovering over it,
     * but the text itself belongs to the Help document so its position remains
     * stable and discoverable.
     */
    lines.push(Line::styled(
        TIPS_LINK_TEXT,
        Style::default().fg(Color::Rgb(90, 150, 235)),
    ));

    push_title(&mut lines, "The Interface", theme);

    push_section(&mut lines, "Search Field", theme);

    push_paragraph(
        &mut lines,
        "The Search field is always ready for input. Typing begins filtering or \
    searching immediately, while the current mode is shown in brackets beside \
    the field. Backspace deletes the character before the caret and never changes \
    the active directory. The visible caret may be moved one character at a time \
    (Ctrl+Left and Ctrl+Right), sent to the beginning or end of the query (Ctrl+ Home \
    and Ctrl+End), or cleared together with the complete query (Ctrl+U).",
        text_width,
        theme,
    );

    push_section(&mut lines, "Details Panel", theme);

    push_paragraph(
        &mut lines,
        "The Details panel presents information about the selected entry, including \
        its name, classification, size, modification date, age, owner, permissions, \
        and full path. It may be shown or hidden at any time (Ctrl+D).",
        text_width,
        theme,
    );

    push_section(&mut lines, "Metadata Panel", theme);

    push_paragraph(
        &mut lines,
        "The Metadata panel appears beside the main listing and provides optional \
        Permissions, Size, Date, and User columns. The complete panel may be shown \
        or hidden (Alt+M), while the individual columns are controlled separately \
        with F7, F8, F9, and F10. Its width adapts to the columns currently in use.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Main Listing", theme);

    push_paragraph(
        &mut lines,
        "The main listing is where files, directories, symbolic links, search \
        results, and Tree branches are displayed. The highlighted row is the \
        current selection, while the parent control in the panel title returns to \
        the preceding directory or search root. Hidden entries may be revealed or \
        concealed (Alt+H), while Hidden Only mode restricts the listing to hidden \
        content (F6). File and directory icons may be shown or hidden (F3), and \
        classified filename colors may be toggled independently (F12).",
        text_width,
        theme,
    );

    push_section(&mut lines, "Selection Panel", theme);

    push_paragraph(
        &mut lines,
        "The Selection panel shows the classification and complete path of the \
        currently highlighted entry, making long paths available even when they \
        cannot fit inside the main listing. It may be shown or hidden (Ctrl+S).",
        text_width,
        theme,
    );

    push_section(&mut lines, "Footer", theme);

    push_paragraph(
        &mut lines,
        "The footer provides an immediate reminder of frequently used controls and \
        displays the current state of important interface options. Its contents \
        adapt to the active view and available features rather than attempting to \
        reproduce every command.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Shortcut Legend", theme);

    push_paragraph(
        &mut lines,
        "The Shortcut Legend is the quick reference for Scry's keyboard and mouse \
        controls (?). It is intended for rapid lookup, while this Help window \
        explains the interface, features, and workflows in fuller detail.",
        text_width,
        theme,
    );

    push_title(&mut lines, "Browsing and Navigation", theme);

    push_section(&mut lines, "List Mode", theme);

    push_paragraph(
        &mut lines,
        "List mode presents the contents of the active directory as a straightforward \
        collection of entries. The selection may be moved through the listing, \
        directories may be entered, and files may be opened with their appropriate \
        application. Returning to the parent restores previously retained positions \
        where possible, so moving back through the filesystem does not always begin \
        again at the top of each directory.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Tree Mode", theme);

    push_paragraph(
        &mut lines,
        "Tree mode displays directories as expandable branches and makes the \
    relationship between parents and descendants visible. It may be enabled or \
    disabled at any time (Ctrl+T). Ctrl+Right expands the selected directory, while \
    Ctrl+Left collapses an open branch or moves the selection to its parent. Alt+E \
    expands or collapses every branch represented by the current Tree, providing a \
    quick way to open a complete hierarchy or fold a large result back down. Enter \
    makes the selected directory the new active root, closing the former hierarchy \
    behind it.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Sorting", theme);

    push_paragraph(
        &mut lines,
        "Entries may normally be sorted by Name, Size, Date, or Type, with the \
    current choice shown in the main listing title. The available sort modes \
    may be cycled (Ctrl+O), and the direction may be reversed independently \
    (Ctrl+R). Flat Fuzzy results are instead ordered by relevance, so ordinary \
    sorting controls are unavailable in Fuzzy List mode. Fuzzy Tree mode still \
    uses the selected sort mode to arrange siblings within each branch. \
    Directories remain grouped above ordinary files where structural sorting \
    applies.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Hidden Entries and Hidden Only", theme);

    push_paragraph(
        &mut lines,
        "Hidden files and directories may be revealed alongside ordinary content \
        without leaving the current location (Alt+H). Hidden Only mode instead \
        restricts the visible result corpus to hidden content (F6). An entry belongs \
        to hidden content when its own name begins with a dot or when any directory \
        in its path begins with a dot. A file such as .config/scry/scry.toml therefore \
        remains part of Hidden Only even though scry.toml itself is not dot-prefixed. \
        Pressing F6 again returns to the ordinary hidden-entry policy.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Navigation", theme);

    push_paragraph(
        &mut lines,
        "The selection may be moved one entry at a time with Up and Down, or one \
        visible page at a time with PgUp and PgDn. Ctrl+PgUp and Ctrl+PgDn move \
        ten visible pages at once, providing faster travel through exceptionally \
        long listings and expanded Trees; releasing Ctrl immediately returns \
        paging to its normal one-page movement. Home and End select the first or \
        last visible entry, while the mouse wheel may also be used for ordinary \
        scrolling.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Very Large Trees", theme);

    push_paragraph(
        &mut lines,
        "A query-less Tree may represent an extremely large filesystem hierarchy, \
        particularly when Alt+E is used to expand every represented branch. Scry \
        therefore uses configurable safeguards to very large Tree displays. \
        The config variable 'expand_all_warning_rows' controls when complete expansion requires \
        confirmation, while 'max_visible_tree_rows' sets the maximum number of Tree \
        rows that may be visible at one time. This ceiling limits only the displayed \
        Tree; it does not restrict indexing, recursive searching, or the number of \
        filesystem entries available to Scry.",
        text_width,
        theme,
    );

    lines.push(Line::raw(""));

    push_paragraph(
        &mut lines,
        "When an expansion would exceed the configured maximum, Scry keeps the \
        existing valid Tree instead of opening an oversized representation. Manual \
        branches may continue to be expanded while sufficient visible-row capacity \
        remains, and collapsing open branches frees that capacity for expansion \
        elsewhere. Alt+E may always be used to collapse a fully expanded Tree. \
        When a saved session is restored, Scry does not automatically recreate a \
        previous query-less Expand All state; instead, the Tree remains collapsed \
        except for the ancestor path required to reveal the restored selection.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Browsing Restrictions", theme);

    push_paragraph(
        &mut lines,
        "Very large Trees are subject to a hard visible-row limit so Scry cannot \
        accidentally open a hierarchy that is too large to browse safely. Alt+E \
        may always collapse an expanded Tree, but the opposite is not always true: \
        complete expansion, manual branch expansion, or another guarded Tree \
        transition may be refused when it would exceed max_visible_tree_rows. \
        The default maximum is 250000 visible Tree rows and may be changed in \
        ~/.config/scry/scry.toml.",
        text_width,
        theme,
    );

    lines.push(Line::raw(""));

    push_paragraph(
        &mut lines,
        "Some large-Tree dialogs are shown only once during a Scry session. After \
        the explanation has already been shown, the same restriction may still \
        apply without opening the dialog again; Scry instead reports the refusal \
        through its notification area while leaving the last valid Tree unchanged. \
        Collapsing branches reduces the visible row count and may make room for \
        further expansion elsewhere.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Icons and File Colors", theme);

    push_paragraph(
        &mut lines,
        "Optional file and directory icons may be shown or hidden (F3). Classified \
        filename colors may be toggled independently (F12), assigning a bright \
        visual family to ordinary files according to Scry's established file \
        classification. Directories and symbolic links retain their structural \
        colors. Both optional visual features are disabled by default.",
        text_width,
        theme,
    );

    push_title(&mut lines, "Searching", theme);

    push_section(&mut lines, "Normal Searching", theme);

    push_paragraph(
        &mut lines,
        "Normal searching filters the entries within the current directory as text \
        is entered. Searches are case-insensitive by default and may match either \
        filenames or their surrounding paths. Ordinary text is applied immediately, \
        and multiple unsigned words are interpreted together as one exact phrase. \
        A query consisting only of a single dot is treated as having no text \
        filter.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Recursive Searching", theme);

    push_paragraph(
        &mut lines,
        "Recursive searching extends the current scope to every descendant beneath \
        the active directory (Alt+R). Local filesystems are scanned in the \
        background, so a large directory tree may require some time before its \
        complete search corpus is available. Exact results may appear progressively \
        as entries are discovered. In Hidden Only mode, Scry still traverses ordinary \
        directories when necessary to discover hidden directories deeper in the \
        hierarchy, but those ordinary traversal paths are not published as results.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Fuzzy Searching", theme);

    push_paragraph(
        &mut lines,
        "Fuzzy searching favors useful approximate matches instead of requiring an \
        exact substring (Ctrl+F). It can recognize abbreviations, omitted \
        characters, small typing mistakes, and adjacent transpositions; for \
        example, \"hlp\" and \"hlep\" may both locate \"help\". Results are ordered \
        by relevance so the strongest matches appear first.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Fuzzy and Recursive Searching", theme);

    push_paragraph(
        &mut lines,
        "Fuzzy and Recursive searching combines approximate matching with the full \
        descendant scope of the active directory. It is enabled by using Fuzzy \
        searching (Ctrl+F) together with Recursive searching (Alt+R). Because the \
        complete corpus may be very large, Scry retains and displays only the \
        strongest ranked results rather than presenting every possible match in \
        ordinary sort order.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Search Result Limits", theme);

    push_paragraph(
        &mut lines,
        "Fuzzy searches retain a configurable number of the strongest direct \
    matches, while Exact Tree searches use a separate configurable direct-match \
    limit. These limits apply to matching entries before a Tree is constructed. \
    Tree mode then adds the ancestor directories required to show those matches \
    in their filesystem context, so the displayed Tree may contain more rows than \
    the configured match limit. The additional number depends on where the \
    matching entries are located and therefore is not a fixed amount.",
        text_width,
        theme,
    );

    lines.push(Line::raw(""));

    push_paragraph(
        &mut lines,
        "In Fuzzy+Recursive Tree mode, a query containing only structural selectors \
    such as \"type:code\" or \"ext:rs\" has no filename or path text to score \
    approximately. Such a query therefore uses Exact Tree matching semantics and \
    the configured exact_tree_match_limit so toggling Exact/Fuzzy does not change \
    the hierarchy. As soon as textual search input is added, as in \
    \"type:code test\", normal Fuzzy ranking resumes and fuzzy_result_limit becomes \
    the direct-match limit. Both settings are documented in scry.toml.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Query-language Reference", theme);

    push_paragraph(
        &mut lines,
        "The sections below explain Scry's compact modifiers and Boolean query \
    language in detail. Open the Shortcut Legend with ? for a complete \
    quick-reference list of every query form, every accepted type: value, and \
    all supported aliases. That reference is generated from the same definitions \
    used by the query parser, so the documented values remain synchronized with \
    the search engine.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Type Modifiers", theme);

    push_paragraph(
        &mut lines,
        "The type: modifier restricts results by classification. General categories \
        such as \"type:directory\", \"type:source\", and \"type:image\" may be used \
        alongside dedicated language classes such as \"type:python\" and \
        \"type:asm\". A modifier may be followed by ordinary text, as in \
        \"type:source index\", to require both the classification and the remaining \
        search phrase.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Extension Modifiers", theme);

    push_paragraph(
        &mut lines,
        "The ext: modifier examines the actual file extension rather than searching \
        for text anywhere in the filename or path. For example, \"ext:jpg\" matches \
        files whose extension is .jpg, while \"type:image ext:tif\" requires both \
        an image classification and the exact .tif extension. A leading dot is \
        optional, so \"ext:rs\" and \"ext:.rs\" are equivalent.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Inclusive and Exclusive Terms", theme);

    push_paragraph(
        &mut lines,
        "Terms beginning with + are required, while terms beginning with - are \
        excluded. Scry first interprets a signed term as a known type or language, \
        then as a known extension, and otherwise as filename or path text. Thus \
        \"+python\" requires Python files, \"-java\" excludes Java files, \"+jpg\" \
        requires the .jpg extension, and \"-.cache\" rejects paths containing \
        .cache. Every positive term must match, while a match against any negative \
        term removes the entry.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Boolean Expressions", theme);

    push_paragraph(
        &mut lines,
        "Advanced searches may combine operands with the word operators AND, OR, and \
    NOT. Operators are recognized without regard to capitalization, but writing \
    them in uppercase makes longer expressions easier to read. Parentheses may be \
    used to group related terms. For example, \"rust AND test\" requires both \
    operands, \"rust OR python\" accepts either, and \
    \"type:source AND NOT target\" finds source files whose paths do not match \
    target.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Boolean Precedence", theme);

    push_paragraph(
        &mut lines,
        "Boolean expressions follow the precedence order NOT, then AND, then OR. Thus \
    \"rust OR python AND test\" is interpreted as \
    \"rust OR (python AND test)\". Parentheses may be added whenever another \
    grouping is intended, such as \"(rust OR python) AND test\". Incomplete live \
    expressions remain harmless while they are being typed and begin filtering \
    only after they form a valid expression.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Case-sensitive Searching", theme);

    push_paragraph(
        &mut lines,
        "Searching is case-insensitive by default. The directive type:sensitive makes \
    textual operands appearing after it case-sensitive for the remainder of the \
    query. For example, \"type:sensitive README\" distinguishes README from \
    readme, while \"rust OR type:sensitive Makefile\" keeps the earlier rust \
    operand insensitive and applies exact capitalization to Makefile. Type and \
    extension classifications themselves remain normalized rather than becoming \
    case-sensitive.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Searching over SSH", theme);

    push_paragraph(
        &mut lines,
        "Ordinary searches over SSH filter the entries already loaded from the \
        current remote directory. Recursive remote searches use the host's \
        persistent index, while the active remote directory continues to define \
        the visible search scope. Hidden Only searches over the recursive remote \
        corpus require an index built with hidden entries included. Existing indexes \
        remain readable after compatible Scry upgrades, although rebuilding an older \
        index may be necessary before newly introduced file classifications become \
        available.",
        text_width,
        theme,
    );

    push_title(&mut lines, "SSH Connections", theme);

    push_section(&mut lines, "Connection Window", theme);

    push_paragraph(
        &mut lines,
        "The Connection window manages reusable SSH profiles and may be opened at \
        any time (F4). Each profile may contain a profile name, host, username, \
        port, identity file, and starting directory. Save stores the current \
        profile locally, Connect opens the selected connection, Delete removes the \
        stored profile, Disconnect returns to the local filesystem, and Cancel \
        closes the window without connecting.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Remote Browsing", theme);

    push_paragraph(
        &mut lines,
        "Remote files and directories behave much like their local counterparts: \
        directories may be entered, listings may be sorted and searched, and the \
        same List and Tree views remain available. A remote file must first be \
        transferred into Scry's local cache before it can be opened with a desktop \
        application or terminal program.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Marking Files", theme);

    push_paragraph(
        &mut lines,
        "While browsing through SSH, files may be marked for a later batch download \
        with Ctrl+Space. Pressing Ctrl+Space again on an already marked file removes \
        its mark. Marks are independent from the ordinary highlighted row and remain \
        attached to their full paths while the user filters results, changes \
        directories, switches between List and Tree views, or restores a saved SSH \
        session. Directories cannot currently be marked. Alt+U clears every marked \
        file. Marking and clearing marks are unavailable during local browsing. \
        Existing marks do not change Enter behavior: Enter always activates only the \
        currently highlighted entry, while Alt+D starts the marked batch download.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Batch Downloads", theme);

    push_paragraph(
        &mut lines,
        "While browsing through SSH, Alt+D downloads every marked file as one batch. \
    By default, the files are gathered directly inside a newly created local \
    batch directory, making files selected from different remote locations \
    immediately accessible in one place. Duplicate filenames receive safe numeric \
    suffixes rather than overwriting one another. Remote directory paths may \
    instead be retained by enabling preserve_hierarchy in scry.toml or launching \
    Scry with --preserve-hierarchy. When Scry entered SSH through the F4 Connection \
    window, the download is placed beneath the saved local browsing directory. \
    When Scry was launched directly with --ssh, the process's launch directory is \
    used instead.",
        text_width,
        theme,
    );

    push_section(&mut lines, "File Transfers", theme);

    push_paragraph(
        &mut lines,
        "Remote transfers are written through temporary partial files so an \
    interrupted download is not mistaken for a complete local copy. A single \
    remote file activated with Enter is transferred into Scry's private cache \
    before it is opened. A marked batch started with Alt+D is instead written \
    into a visible local download directory. During a batch, the transfer window \
    identifies the current file and reports aggregate bytes, completion percentage, \
    elapsed time, speed, and file position within the queue. When the batch \
    finishes, the final window reports the number of files downloaded, failures \
    where applicable, the destination directory, total transferred size, elapsed \
    time, and average speed. Failed files remain marked so they may be retried. \
    A completed or failed result remains visible until it is acknowledged with \
    Enter or Escape.",
        text_width,
        theme,
    );

    push_title(&mut lines, "Remote Index", theme);

    push_section(&mut lines, "Purpose", theme);

    push_paragraph(
        &mut lines,
        "Recursive searching over SSH uses a persistent remote index instead of \
        asking SFTP to rescan the host for every query. The index records the \
        remote filesystem once and stores the result locally, allowing later \
        recursive searches to respond quickly even when the host contains hundreds \
        of thousands or millions of entries.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Building an Index", theme);

    push_paragraph(
        &mut lines,
        "The Remote Index Builder may be opened manually (F5). A Standard build \
        records ordinary entries, while Include Hidden also records dot-prefixed \
        files and directories together with the descendants of hidden directories. \
        This extended corpus is required for recursive Hidden Only searches. After \
        the build has started, it continues in the background and reports its progress \
        while the rest of Scry remains available for browsing.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Loading and Rebuilding", theme);

    push_paragraph(
        &mut lines,
        "A completed index is stored locally and reused automatically for later \
        connections to the same remote host, account, and port. Compatible older \
        indexes remain readable, but rebuilding may be useful after Scry gains new \
        file classifications or indexing behavior. An older index preserves the \
        classifications written when it was created, while a rebuilt index records \
        the richer information available in the current version.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Scope and Safety", theme);

    push_paragraph(
        &mut lines,
        "A remote index represents the host filesystem beginning at /, but the \
        currently active remote directory limits which part of that index appears \
        in search results. Volatile system trees such as /proc, /sys, /dev, and \
        /run are skipped during indexing because they contain temporary kernel and \
        device information rather than ordinary files intended for browsing.",
        text_width,
        theme,
    );

    push_title(&mut lines, "Opening Files", theme);

    push_section(&mut lines, "Opening Behavior", theme);

    push_paragraph(
        &mut lines,
        "Directories are entered directly, while executable files are launched in a \
    terminal. Ordinary files are opened with the desktop's default application, \
    and text files may fall back to a terminal editor when no suitable desktop \
    opener is available. Remote files are first transferred into Scry's local \
    cache and are then opened in the same way as local files.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Keeping Scry Open", theme);

    push_paragraph(
        &mut lines,
        "Scry remains open after successfully launching a file by default, allowing \
    browsing to continue while the external application runs. Set exit_on_open \
    to true in scry.toml or launch with --exit-on-open when Scry should close \
    after a file has been opened successfully. Directory navigation and failed \
    open attempts never trigger this automatic exit.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Disabling File Opening", theme);

    push_paragraph(
        &mut lines,
        "External file opening may be disabled with allow_file_opening = false in \
    scry.toml or for one launch with --no-open. This affects ordinary file \
    activation only; directories may still be entered and browsed normally. \
    --no-open and --exit-on-open are mutually exclusive because one disables \
    the action that the other waits to complete.",
        text_width,
        theme,
    );

    push_title(&mut lines, "Deletion", theme);

    push_paragraph(
        &mut lines,
        "Deletion is disabled by default and must be enabled in Scry's configuration \
    before the Delete key becomes active. Deletion is currently available only \
    for local entries; remote files and directories cannot be removed through \
    SSH. Every request opens a confirmation window with Cancel selected by \
    default. Files, directories, and symbolic links are first moved to hidden \
    staged paths beside their original locations. A symbolic link is handled as \
    a link and is never followed into its target. Scry also refuses dangerous \
    targets such as the filesystem root, the current browsing root, or paths \
    outside the active root. Press Ctrl+Z to restore the most recently staged \
    deletion during the current session. Remaining staged entries are removed \
    permanently when Scry exits cleanly, while interrupted deletion sessions can \
    be recovered from the deletion journal when Scry starts again.",
        text_width,
        theme,
    );

    push_title(&mut lines, "Session Restoration", theme);

    push_section(&mut lines, "Enabling Restoration", theme);

    push_paragraph(
        &mut lines,
        "Session restoration is disabled by default. It may be enabled permanently \
with restore_session = true in the [session] section of scry.toml, or for one \
launch with --restore-session. When enabled, Scry saves its stable browser \
state during a normal shutdown and attempts to restore it the next time Scry \
is launched without an explicit replacement source.",
        text_width,
        theme,
    );

    push_paragraph(
        &mut lines,
        "Restored session settings override matching browser and display defaults \
from scry.toml. Explicit command-line options override both.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Restored State", theme);

    push_paragraph(
        &mut lines,
        "A restored session may recover the local or SSH source, active directory, \
    selected entry, viewport position, search query, List or Tree view, Exact or \
    Fuzzy mode, recursive scope, entry filter, sorting, hidden-entry state, icons, \
    panels, and metadata columns. Marked SSH files are also restored, allowing an \
    interrupted browsing session to reconnect later and resume its planned batch \
    download.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Command-line Precedence", theme);

    push_paragraph(
        &mut lines,
        "Explicit startup choices take precedence over saved state. Supplying a local \
    PATH or --ssh target selects that source instead of the saved one, while \
    command-line view, search, filter, query, opening, and metadata options \
    override corresponding restored values for the current launch. This allows a \
    saved session to provide convenient defaults without preventing deliberate \
    one-time startup choices.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Storage and Failure Safety", theme);

    push_paragraph(
        &mut lines,
        "Session data is stored as session.json beneath \
    $XDG_STATE_HOME/scry when XDG_STATE_HOME is set, otherwise beneath \
    ~/.local/state/scry. The file is published atomically through a temporary \
    part file. Passwords and temporary interface state are not stored. If a saved \
    source cannot be reopened or an SSH host cannot be reached, Scry falls back \
    safely rather than treating an incomplete restoration as a valid replacement \
    for the previous saved session.",
        text_width,
        theme,
    );

    push_title(&mut lines, "Configuration", theme);

    push_paragraph(
        &mut lines,
        "Scry reads its startup settings from scry.toml in the user's configuration \
    directory. The file controls display choices, browser behavior, optional \
    features, session restoration, SSH behavior, and marked-download hierarchy, \
    while command-line options take precedence for the current launch. Missing or \
    invalid values fall back to safe built-in defaults instead of preventing Scry \
    from starting. A documented configuration template may be generated with \
    --generate-config, complete command-line usage is available through \
    \"scry --help\", and this full manual may also be printed with \
    \"scry --manual\".",
        text_width,
        theme,
    );

    push_title(&mut lines, "Themes", theme);

    push_paragraph(
        &mut lines,
        "Scry's appearance is selected through the configuration file, with theme \
        definitions stored in Scry's theme directory. A theme may assign colors to \
        interface frames, ordinary files and directories, file classifications, \
        permission characters, icons, selections, messages, and other visual \
        elements. Missing themes, malformed files, and invalid individual color \
        values fall back safely to Scry's built-in defaults so a broken theme \
        cannot prevent the application from starting.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Creating Custom Themes", theme);

    push_paragraph(
        &mut lines,
        "Users may create their own themes by copying one of Scry's existing theme \
        files, renaming the copy, and changing its color values. Starting from an \
        existing theme is recommended because it provides the complete set of \
        available color settings and makes it easy to see how a theme is structured. \
        Custom themes should be placed in ~/.config/scry/themes and can then be \
        selected with the top-level theme setting in scry.toml. Keeping the original \
        theme files unchanged also makes it easy to return to Scry's supplied themes \
        or use them again as clean starting points.",
        text_width,
        theme,
    );

    push_title(&mut lines, "Keyboard and Mouse Use", theme);

    push_paragraph(
        &mut lines,
        "Scry supports both keyboard and mouse navigation throughout the interface. \
    Mouse actions include selecting entries, activating them with a double \
    click, dragging scrollbars through long listings, and clicking available \
    controls in supported windows. The complete keyboard and mouse bindings \
    are available in the Shortcut Legend (?).",
        text_width,
        theme,
    );

    /*
     * ========== TIPS SECTION ==========
     */
    push_title(&mut lines, "Tips", theme);

    push_tip_paragraph(
        &mut lines,
        "# For a compact reminder of keyboard controls and query syntax, open the Shortcut \
    Legend with ? instead of searching through this complete Help document.",
        text_width,
    );

    lines.push(Line::raw(""));

    push_tip_paragraph(
        &mut lines,
        "# When browsing a very large listing, click and hold the scrollbar track above \
    or below the thumb to move through the entries rapidly one page at a time. You may \
    also click on either side of the thumb to move a single page in that direction.",
        text_width,
    );

    lines.push(Line::raw(""));

    push_tip_paragraph(
        &mut lines,
        "# Icons and classified filename colors are optional. To enable them permanently, \
    edit ~/.config/scry/scry.toml and set show_icons = true and/or \
    show_file_colors = true.",
        text_width,
    );

    lines.push(Line::raw(""));

    push_tip_paragraph(
        &mut lines,
        "# Did you know that you can enable icons in-app by pressing F3, and file colors with F12?",
        text_width,
    );

    lines.push(Line::raw(""));

    push_tip_paragraph(
        &mut lines,
        "# When in HiddenOnly mode (F6), you may sometimes end up inside directories without \
        any dot-entries in it. That means, there will be no visible entry to select and open/enter, which \
        might make you feel trapped. Just disable HiddenOnly by pressing F6 to see the selectable \
        entries again.",
        text_width,
    );

    lines.push(Line::raw(""));

    push_tip_paragraph(
        &mut lines,
        "# If you have lots of search results, and you have enabled file colors, and \
        you find that it is difficult to see the highlighting in your search results, try toggling \
        file colors off (F12) temporarily for clarity.",
        text_width,
    );

    lines.push(Line::raw(""));

    push_tip_paragraph(
        &mut lines,
        "# You may want to have Recursive search mode enabled (Alt+R) when searching. \
        Recursive mode includes all forward directories from the current root. Sometimes, you may \
        have had the intention to search all subdirectories, but there will only be results from \
        the current root unless you are in Recursive mode.",
        text_width,
    );

    lines.push(Line::raw(""));

    push_tip_paragraph(
        &mut lines,
        "# When a directory wears the right arrow (→) after its name, it means the directory \
        contains other entries. When there, instead of a right arrow, is a slash (/), there are no \
        entries inside. However, if you, while browsing, enter a → dir, and you find it empty, it is \
        certain to have hidden entries inside. Enable Hidden entries to gain access to them.",
        text_width,
    );

    lines.push(Line::raw(""));

    /*
     * Leave one empty line below the final paragraph so the document does not
     * end directly against the bottom edge.
     */
    lines.push(Line::raw(""));

    lines.push(Line::styled(
        TOP_LINK_TEXT,
        Style::default().fg(Color::Rgb(90, 150, 235)),
    ));

    lines.push(Line::raw(""));

    lines.push(
        Line::styled(
            "↑/↓ scroll  PgUp/PgDn page  Esc/F1 closes",
            Style::default().fg(Color::Rgb(75, 80, 92)),
        )
        .alignment(Alignment::Center),
    );

    lines
}

/*
 * Print the same document used by the F1 Help window as plain text.
 *
 * Styling is deliberately discarded. The resulting output is safe to redirect
 * into files, pipe through pagers, or open in an external text editor.
 */
pub fn print_manual(theme: &Theme, text_width: usize) -> io::Result<()> {
    let lines = content(theme, text_width);

    let stdout = io::stdout();

    let mut output = stdout.lock();

    for line in lines {
        for span in line.spans {
            write!(output, "{}", span.content)?;
        }

        writeln!(output)?;
    }

    Ok(())
}

fn push_title(lines: &mut Vec<Line<'static>>, title: &str, theme: &Theme) {
    /*
     * Main document headings receive the strongest visual separation.
     */
    if !lines.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::raw(""));
    }

    lines.push(Line::styled(
        title.to_string(),
        Style::default()
            .fg(theme.ui.query)
            .add_modifier(Modifier::BOLD),
    ));

    lines.push(Line::raw(""));
}

fn push_section(lines: &mut Vec<Line<'static>>, title: &str, theme: &Theme) {
    /*
     * Add one separating row only when the previous line is not
     * already blank. This prevents an oversized gap when a subtitle
     * follows a main title, while still separating it from ordinary
     * paragraph text.
     */
    if lines.last().is_some_and(|line| line.width() > 0) {
        lines.push(Line::raw(""));
    }

    lines.push(Line::styled(
        format!("  {}", title),
        Style::default()
            .fg(theme.ui.classification)
            .add_modifier(Modifier::BOLD),
    ));

    lines.push(Line::raw(""));
}

fn push_intro_paragraph(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    text_width: usize,
    theme: &Theme,
) {
    for wrapped_line in wrap_text(text, text_width) {
        lines.push(Line::styled(
            wrapped_line,
            Style::default()
                .fg(theme.ui.muted)
                .add_modifier(Modifier::DIM),
        ));
    }
}

fn push_paragraph(lines: &mut Vec<Line<'static>>, text: &str, text_width: usize, theme: &Theme) {
    for wrapped_line in wrap_text(text, text_width) {
        lines.push(Line::styled(
            wrapped_line,
            Style::default().fg(theme.ui.file),
        ));
    }
}

fn push_tip_paragraph(lines: &mut Vec<Line<'static>>, text: &str, text_width: usize) {
    for wrapped_line in wrap_text(text, text_width) {
        lines.push(Line::styled(
            wrapped_line,
            Style::default().fg(COLOR_TIP_TEXT),
        ));
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);

    let mut wrapped_lines = Vec::new();

    for source_paragraph in text.split('\n') {
        if source_paragraph.trim().is_empty() {
            wrapped_lines.push(String::new());

            continue;
        }

        let mut current_line = String::new();

        for word in source_paragraph.split_whitespace() {
            /*
             * The first word can always enter an empty line.
             */
            if current_line.is_empty() {
                current_line.push_str(word);

                continue;
            }

            let proposed_width = current_line
                .chars()
                .count()
                .saturating_add(1)
                .saturating_add(word.chars().count());

            if proposed_width <= width {
                current_line.push(' ');

                current_line.push_str(word);
            } else {
                wrapped_lines.push(current_line);

                current_line = word.to_string();
            }
        }

        if !current_line.is_empty() {
            wrapped_lines.push(current_line);
        }
    }

    wrapped_lines
}
