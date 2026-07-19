// SPDX-License-Identifier: BSD-3-Clause

use ratatui::{
    style::{Modifier, Style},
    text::Line,
};

use crate::themes::Theme;

pub fn content(theme: &Theme, text_width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    push_paragraph(
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
        the field. The visible caret may be moved one character at a time \
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

    push_section(&mut lines, "File Transfers", theme);

    push_paragraph(
        &mut lines,
        "Remote transfers are written through temporary partial files so an \
        interrupted download is not mistaken for a complete local copy. The \
        transfer window reports the filename, transferred bytes, completion \
        percentage, elapsed time, and average speed while the operation is active. \
        When the transfer finishes or fails, the final result remains visible \
        until it is acknowledged with Enter or Escape.",
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

    push_title(&mut lines, "Configuration", theme);

    push_paragraph(
        &mut lines,
        "Scry reads its startup settings from scry.toml in the user's configuration \
        directory. The file controls default display choices, browser behavior, \
        optional features, and SSH settings, while command-line options take \
        precedence for the current launch. Missing or invalid values fall back to \
        safe built-in defaults instead of preventing Scry from starting. Complete \
        command-line usage and available options are documented separately through \
        \"scry --help\".",
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
