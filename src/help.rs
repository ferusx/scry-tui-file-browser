// SPDX-License-Identifier: BSD-3-Clause

use std::io::{self, Write};

use ratatui::{
    style::{Modifier, Style},
    text::Line,
};

use crate::themes::Theme;

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

    push_title(&mut lines, "The Interface", theme);

    push_section(&mut lines, "Search Field", theme);

    push_paragraph(
        &mut lines,
        "The Search field is always ready for input. Typing begins filtering or \
    searching immediately, while the current mode is shown in brackets beside \
    the field. Backspace deletes the character before the caret and never changes \
    the active directory. The visible caret may be moved one character at a time \
    (Ctrl+Left and Ctrl+Right), sent to the beginning or end of the query \
    (Ctrl+Home and Ctrl+End), or cleared together with the complete query \
    (Ctrl+U).",
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
        the preceding directory or search root. File and directory icons may be \
        shown or hidden (F3), and hidden entries may be revealed or concealed \
        (Alt+H).",
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
        controls (Ctrl+!). It is intended for rapid lookup, while this Help window \
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
        disabled at any time (Ctrl+T). Right expands the selected directory, while \
        Left collapses an open branch or moves the selection to its parent. Enter \
        makes the selected directory the new active root, closing the former \
        hierarchy behind it.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Sorting", theme);

    push_paragraph(
        &mut lines,
        "Entries may be sorted by Name, Size, Date, or Type, with the current choice \
        shown in the main listing title. The available sort modes may be cycled \
        (Ctrl+O), and the direction may be reversed independently (Ctrl+R). \
        Directories remain grouped above ordinary files so navigation stays \
        predictable.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Hidden Entries and Icons", theme);

    push_paragraph(
        &mut lines,
        "Hidden files and directories may be revealed or concealed without leaving \
        the current location (Alt+H). Optional file and directory icons may also be \
        shown or hidden (F3), allowing the listing to favor either visual \
        classification or maximum compatibility with terminals and fonts that do \
        not provide icon support.",
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
        as entries are discovered.",
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

    push_section(&mut lines, "Query-language Reference", theme);

    push_paragraph(
        &mut lines,
        "The sections below explain Scry's compact modifiers and Boolean query \
    language in detail. Open the Shortcut Legend with Ctrl+! for a complete \
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

    push_section(&mut lines, "Pending Modifiers", theme);

    push_paragraph(
        &mut lines,
        "A modifier being written at the end of the query remains pending until it \
        is committed. This prevents partial terms such as \"-e\", \"-ex\", or \
        \"ext:pn\" from repeatedly changing a large result set while they are still \
        being entered. Press Space to begin another term, or Enter to commit the \
        pending modifier without activating the selected entry. Pressing Enter \
        again performs the normal selection action.",
        text_width,
        theme,
    );

    push_section(&mut lines, "Searching over SSH", theme);

    push_paragraph(
        &mut lines,
        "Ordinary searches over SSH filter the entries already loaded from the \
        current remote directory. Recursive remote searches use the host's \
        persistent index, while the active remote directory continues to define \
        the visible search scope. Existing indexes remain readable after compatible \
        Scry upgrades, although rebuilding an older index may be necessary before \
        newly introduced file classifications become available.",
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
    file. Marking and clearing marks are unavailable during local browsing.",
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
        records ordinary entries, while Include Hidden also records files and \
        directories whose names begin with a dot. After the build has started, it \
        continues in the background and reports its progress while the rest of \
        Scry remains available for browsing.",
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
        default. Ordinary files and symbolic links are removed directly, while \
        directories and all of their contents are removed recursively. A symbolic \
        link is deleted as a link and is never followed into its target. Scry also \
        refuses dangerous targets such as the filesystem root, the current browsing \
        root, or paths outside the active root. Confirmed deletion is permanent and \
        does not use a trash or recovery area.",
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

    push_title(&mut lines, "Keyboard and Mouse Use", theme);

    push_paragraph(
        &mut lines,
        "Scry supports both keyboard and mouse navigation throughout the interface. \
        Mouse actions include selecting entries, activating them with a double \
        click, using the middle button to return to a parent or collapse a Tree \
        branch, dragging scrollbars through long listings, and clicking available \
        controls in supported windows. The complete keyboard and mouse bindings \
        are available in the Shortcut Legend (Ctrl+!).",
        text_width,
        theme,
    );

    /*
     * Leave one empty line below the final paragraph so the document does not
     * end directly against the bottom edge.
     */
    lines.push(Line::raw(""));

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
