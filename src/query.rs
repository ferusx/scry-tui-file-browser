// SPDX-License-Identifier: BSD-3-Clause

use crate::classify::{FileClass, classify_extension};
use crate::scan::FileEntry;
use crate::search_index::SearchRecord;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueryTypeFilter {
    File,

    Directory,

    Symlink,

    Executable,

    Source,

    Shell,

    Rust,

    Python,

    C,

    Cpp,

    Java,

    Kotlin,

    JavaScript,

    TypeScript,

    Assembly,

    Lua,

    Ruby,

    Perl,

    Php,

    Go,

    Swift,

    Dart,

    CSharp,

    Scala,

    Groovy,

    R,

    Awk,

    Elixir,

    Erlang,

    FSharp,

    VisualBasic,

    Clojure,

    Zig,

    Nim,

    Crystal,

    Haskell,

    Ocaml,

    Pascal,

    Solidity,

    Vala,

    Web,

    Build,

    Config,

    Data,

    Log,

    Archive,

    Package,

    Document,

    Spreadsheet,

    Presentation,

    Image,

    VectorImage,

    Audio,

    Video,

    Font,

    Database,

    Torrent,

    DesktopEntry,

    Backup,

    Certificate,

    DiskImage,

    Plugin,

    Text,

    Binary,

    Unknown,
}

/*
 * One documented type: classification and every spelling accepted for it.
 *
 * This table is the single authority for both query parsing and the in-app
 * Query Modifier reference. Adding or removing an alias here updates both.
 */
#[derive(Debug, Clone, Copy)]
pub(crate) struct QueryTypeReference {
    pub canonical: &'static str,

    pub aliases: &'static [&'static str],

    filter: QueryTypeFilter,
}

/*
 * Complete set of type: values accepted by Scry.
 *
 * canonical is the preferred spelling shown first in documentation. aliases
 * contains every additional spelling accepted by the parser.
 */
pub(crate) const QUERY_TYPE_REFERENCES: &[QueryTypeReference] = &[
    QueryTypeReference {
        canonical: "file",
        aliases: &[],
        filter: QueryTypeFilter::File,
    },
    QueryTypeReference {
        canonical: "directory",
        aliases: &["dir"],
        filter: QueryTypeFilter::Directory,
    },
    QueryTypeReference {
        canonical: "symlink",
        aliases: &["link"],
        filter: QueryTypeFilter::Symlink,
    },
    QueryTypeReference {
        canonical: "executable",
        aliases: &["exec"],
        filter: QueryTypeFilter::Executable,
    },
    QueryTypeReference {
        canonical: "source",
        aliases: &["code"],
        filter: QueryTypeFilter::Source,
    },
    QueryTypeReference {
        canonical: "shell",
        aliases: &["script", "sh"],
        filter: QueryTypeFilter::Shell,
    },
    QueryTypeReference {
        canonical: "rust",
        aliases: &["rs"],
        filter: QueryTypeFilter::Rust,
    },
    QueryTypeReference {
        canonical: "python",
        aliases: &["py"],
        filter: QueryTypeFilter::Python,
    },
    QueryTypeReference {
        canonical: "c",
        aliases: &[],
        filter: QueryTypeFilter::C,
    },
    QueryTypeReference {
        canonical: "cpp",
        aliases: &["c++", "cplusplus"],
        filter: QueryTypeFilter::Cpp,
    },
    QueryTypeReference {
        canonical: "java",
        aliases: &[],
        filter: QueryTypeFilter::Java,
    },
    QueryTypeReference {
        canonical: "kotlin",
        aliases: &["kt"],
        filter: QueryTypeFilter::Kotlin,
    },
    QueryTypeReference {
        canonical: "javascript",
        aliases: &["js"],
        filter: QueryTypeFilter::JavaScript,
    },
    QueryTypeReference {
        canonical: "typescript",
        aliases: &["ts"],
        filter: QueryTypeFilter::TypeScript,
    },
    QueryTypeReference {
        canonical: "assembly",
        aliases: &["assembler", "asm"],
        filter: QueryTypeFilter::Assembly,
    },
    QueryTypeReference {
        canonical: "lua",
        aliases: &[],
        filter: QueryTypeFilter::Lua,
    },
    QueryTypeReference {
        canonical: "ruby",
        aliases: &["rb"],
        filter: QueryTypeFilter::Ruby,
    },
    QueryTypeReference {
        canonical: "perl",
        aliases: &["pl"],
        filter: QueryTypeFilter::Perl,
    },
    QueryTypeReference {
        canonical: "php",
        aliases: &[],
        filter: QueryTypeFilter::Php,
    },
    QueryTypeReference {
        canonical: "go",
        aliases: &["golang"],
        filter: QueryTypeFilter::Go,
    },
    QueryTypeReference {
        canonical: "swift",
        aliases: &[],
        filter: QueryTypeFilter::Swift,
    },
    QueryTypeReference {
        canonical: "dart",
        aliases: &[],
        filter: QueryTypeFilter::Dart,
    },
    QueryTypeReference {
        canonical: "csharp",
        aliases: &["c#", "cs"],
        filter: QueryTypeFilter::CSharp,
    },
    QueryTypeReference {
        canonical: "scala",
        aliases: &[],
        filter: QueryTypeFilter::Scala,
    },
    QueryTypeReference {
        canonical: "groovy",
        aliases: &[],
        filter: QueryTypeFilter::Groovy,
    },
    QueryTypeReference {
        canonical: "r",
        aliases: &["rscript"],
        filter: QueryTypeFilter::R,
    },
    QueryTypeReference {
        canonical: "awk",
        aliases: &[],
        filter: QueryTypeFilter::Awk,
    },
    QueryTypeReference {
        canonical: "elixir",
        aliases: &["ex"],
        filter: QueryTypeFilter::Elixir,
    },
    QueryTypeReference {
        canonical: "erlang",
        aliases: &["erl"],
        filter: QueryTypeFilter::Erlang,
    },
    QueryTypeReference {
        canonical: "fsharp",
        aliases: &["f#", "fs"],
        filter: QueryTypeFilter::FSharp,
    },
    QueryTypeReference {
        canonical: "visualbasic",
        aliases: &["visual-basic", "vb"],
        filter: QueryTypeFilter::VisualBasic,
    },
    QueryTypeReference {
        canonical: "clojure",
        aliases: &["clj"],
        filter: QueryTypeFilter::Clojure,
    },
    QueryTypeReference {
        canonical: "zig",
        aliases: &[],
        filter: QueryTypeFilter::Zig,
    },
    QueryTypeReference {
        canonical: "nim",
        aliases: &[],
        filter: QueryTypeFilter::Nim,
    },
    QueryTypeReference {
        canonical: "crystal",
        aliases: &["cr"],
        filter: QueryTypeFilter::Crystal,
    },
    QueryTypeReference {
        canonical: "haskell",
        aliases: &["hs"],
        filter: QueryTypeFilter::Haskell,
    },
    QueryTypeReference {
        canonical: "ocaml",
        aliases: &["ml"],
        filter: QueryTypeFilter::Ocaml,
    },
    QueryTypeReference {
        canonical: "pascal",
        aliases: &["pas"],
        filter: QueryTypeFilter::Pascal,
    },
    QueryTypeReference {
        canonical: "solidity",
        aliases: &["sol"],
        filter: QueryTypeFilter::Solidity,
    },
    QueryTypeReference {
        canonical: "vala",
        aliases: &[],
        filter: QueryTypeFilter::Vala,
    },
    QueryTypeReference {
        canonical: "web",
        aliases: &[],
        filter: QueryTypeFilter::Web,
    },
    QueryTypeReference {
        canonical: "build",
        aliases: &[],
        filter: QueryTypeFilter::Build,
    },
    QueryTypeReference {
        canonical: "config",
        aliases: &["configuration"],
        filter: QueryTypeFilter::Config,
    },
    QueryTypeReference {
        canonical: "data",
        aliases: &["structured-data", "structureddata"],
        filter: QueryTypeFilter::Data,
    },
    QueryTypeReference {
        canonical: "log",
        aliases: &["logs"],
        filter: QueryTypeFilter::Log,
    },
    QueryTypeReference {
        canonical: "archive",
        aliases: &[],
        filter: QueryTypeFilter::Archive,
    },
    QueryTypeReference {
        canonical: "package",
        aliases: &["pkg"],
        filter: QueryTypeFilter::Package,
    },
    QueryTypeReference {
        canonical: "document",
        aliases: &["doc"],
        filter: QueryTypeFilter::Document,
    },
    QueryTypeReference {
        canonical: "spreadsheet",
        aliases: &["sheet"],
        filter: QueryTypeFilter::Spreadsheet,
    },
    QueryTypeReference {
        canonical: "presentation",
        aliases: &["slides"],
        filter: QueryTypeFilter::Presentation,
    },
    QueryTypeReference {
        canonical: "image",
        aliases: &["images", "img"],
        filter: QueryTypeFilter::Image,
    },
    QueryTypeReference {
        canonical: "vector",
        aliases: &["vector-image", "vectorimage"],
        filter: QueryTypeFilter::VectorImage,
    },
    QueryTypeReference {
        canonical: "audio",
        aliases: &[],
        filter: QueryTypeFilter::Audio,
    },
    QueryTypeReference {
        canonical: "video",
        aliases: &[],
        filter: QueryTypeFilter::Video,
    },
    QueryTypeReference {
        canonical: "font",
        aliases: &["fonts"],
        filter: QueryTypeFilter::Font,
    },
    QueryTypeReference {
        canonical: "database",
        aliases: &["db"],
        filter: QueryTypeFilter::Database,
    },
    QueryTypeReference {
        canonical: "torrent",
        aliases: &[],
        filter: QueryTypeFilter::Torrent,
    },
    QueryTypeReference {
        canonical: "desktop",
        aliases: &["desktop-entry", "desktopentry"],
        filter: QueryTypeFilter::DesktopEntry,
    },
    QueryTypeReference {
        canonical: "backup",
        aliases: &[],
        filter: QueryTypeFilter::Backup,
    },
    QueryTypeReference {
        canonical: "certificate",
        aliases: &["cert"],
        filter: QueryTypeFilter::Certificate,
    },
    QueryTypeReference {
        canonical: "disk-image",
        aliases: &["diskimage"],
        filter: QueryTypeFilter::DiskImage,
    },
    QueryTypeReference {
        canonical: "plugin",
        aliases: &[],
        filter: QueryTypeFilter::Plugin,
    },
    QueryTypeReference {
        canonical: "text",
        aliases: &[],
        filter: QueryTypeFilter::Text,
    },
    QueryTypeReference {
        canonical: "binary",
        aliases: &["bin"],
        filter: QueryTypeFilter::Binary,
    },
    QueryTypeReference {
        canonical: "unknown",
        aliases: &[],
        filter: QueryTypeFilter::Unknown,
    },
];

/*
 * Complete compact and Boolean query-language reference.
 *
 * The internal Shortcut Legend renders this table directly so every available
 * query form remains discoverable alongside the parser-backed type values.
 */
pub(crate) const QUERY_SYNTAX_REFERENCE: &[(&str, &str)] = &[
    ("text", "Match ordinary filename or path text"),
    ("type:TYPE", "Restrict results to one classification"),
    ("ext:EXT", "Require an exact file extension"),
    ("+TERM", "Require a type, extension, or text term"),
    ("-TERM", "Exclude a type, extension, or text term"),
    ("AND", "Require both Boolean operands"),
    ("OR", "Accept either Boolean operand"),
    ("NOT", "Negate the following Boolean operand"),
    ("( ... )", "Group a Boolean expression"),
    ("type:sensitive", "Make later text operands case-sensitive"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SignedQueryTerm {
    Type(QueryTypeFilter),

    Extension(String),

    Text { value: String, case_sensitive: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QueryHighlightTerm {
    pub value: String,

    pub case_sensitive: bool,
}

/*
 * Optional advanced Boolean expression.
 *
 * This is an additional query language layered above Scry's established
 * compact modifiers. Compact queries continue using type:, ext:, +term, and
 * -term exactly as before.
 */
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BooleanExpression {
    Term(SignedQueryTerm),

    Not(Box<BooleanExpression>),

    And(Box<BooleanExpression>, Box<BooleanExpression>),

    Or(Box<BooleanExpression>, Box<BooleanExpression>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedQuery {
    /*
     * Unsigned ordinary filename/path text.
     */
    text: String,

    /*
     * Explicit broad classification:
     *
     *     type:source
     *     type:image
     */
    type_filter: Option<QueryTypeFilter>,

    /*
     * Explicit extension restriction:
     *
     *     ext:rs
     *     ext:.jpg
     */
    extension_filter: Option<String>,

    /*
     * Positive type and extension selectors form an OR group:
     *
     *     +python +lua
     *
     * Positive ordinary text terms remain cumulative:
     *
     *     +index +test
     */
    include_terms: Vec<SignedQueryTerm>,

    /*
     * Any matching negative term rejects the entry:
     *
     *     -java
     *     -png
     *     -.cache
     */
    exclude_terms: Vec<SignedQueryTerm>,

    /*
     * Present only when AND, OR, NOT, or parentheses selected the advanced
     * Boolean query language.
     */
    boolean_expression: Option<BooleanExpression>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BooleanToken {
    Word(String),

    And,

    Or,

    Not,

    Sensitive,

    LeftParenthesis,

    RightParenthesis,
}

/*
 * Public query entry point.
 *
 * Boolean syntax is selected only when the query contains a recognized word
 * operator or parenthesis. Every other query follows the established compact
 * parser unchanged.
 */
pub(crate) fn parse_query(query: &str) -> ParsedQuery {
    if !contains_boolean_syntax(query) {
        return parse_compact_query(query);
    }

    let Some(boolean_expression) = parse_boolean_expression(query) else {
        /*
         * Search is live. Expressions such as:
         *
         *     rs OR
         *     (
         *     NOT
         *
         * are normal temporary states while typing. Treat them as incomplete
         * rather than turning operator words into ordinary filename text or
         * launching a complete all-entry search.
         */
        return ParsedQuery {
            text: String::new(),

            type_filter: None,

            extension_filter: None,

            include_terms: Vec::new(),

            exclude_terms: Vec::new(),

            boolean_expression: None,
        };
    };

    ParsedQuery {
        /*
         * Boolean text terms are evaluated by the expression itself.
         *
         * They must not also become the global Exact/Fuzzy search text or they
         * would be applied twice with different semantics.
         */
        text: String::new(),

        type_filter: None,

        extension_filter: None,

        include_terms: Vec::new(),

        exclude_terms: Vec::new(),

        boolean_expression: Some(boolean_expression),
    }
}

fn contains_boolean_syntax(query: &str) -> bool {
    if query.contains(['(', ')']) {
        return true;
    }

    query.split_whitespace().any(|token| {
        token.eq_ignore_ascii_case("and")
            || token.eq_ignore_ascii_case("or")
            || token.eq_ignore_ascii_case("not")
    })
}

fn parse_boolean_expression(query: &str) -> Option<BooleanExpression> {
    let tokens = tokenize_boolean_query(query)?;

    if tokens.is_empty() {
        return None;
    }

    let mut parser = BooleanParser::new(tokens);

    let expression = parser.parse_or_expression()?;

    /*
     * A valid expression must consume every token.
     *
     * This catches unmatched closing parentheses and adjacent operands without
     * an operator.
     */
    if parser.has_remaining_tokens() {
        return None;
    }

    Some(expression)
}

fn tokenize_boolean_query(query: &str) -> Option<Vec<BooleanToken>> {
    let mut tokens = Vec::new();

    let mut word = String::new();

    let push_word = |word: &mut String, tokens: &mut Vec<BooleanToken>| {
        if word.is_empty() {
            return;
        }

        let token = if word.eq_ignore_ascii_case("and") {
            BooleanToken::And
        } else if word.eq_ignore_ascii_case("or") {
            BooleanToken::Or
        } else if word.eq_ignore_ascii_case("not") {
            BooleanToken::Not
        } else if word.eq_ignore_ascii_case("type:sensitive") {
            BooleanToken::Sensitive
        } else {
            /*
             * Preserve original capitalization.
             *
             * parse_signed_query_term() will lowercase ordinary insensitive terms,
             * while sensitive terms retain their exact spelling.
             */
            BooleanToken::Word(word.clone())
        };

        tokens.push(token);

        word.clear();
    };

    for character in query.chars() {
        match character {
            '(' => {
                push_word(&mut word, &mut tokens);

                tokens.push(BooleanToken::LeftParenthesis);
            }

            ')' => {
                push_word(&mut word, &mut tokens);

                tokens.push(BooleanToken::RightParenthesis);
            }

            character if character.is_whitespace() => {
                push_word(&mut word, &mut tokens);
            }

            _ => {
                word.push(character);
            }
        }
    }

    push_word(&mut word, &mut tokens);

    if tokens.is_empty() {
        None
    } else {
        Some(tokens)
    }
}

struct BooleanParser {
    tokens: Vec<BooleanToken>,

    position: usize,

    /*
     * Once type:sensitive is encountered, every later textual operand keeps
     * its original capitalization.
     */
    case_sensitive: bool,
}

impl BooleanParser {
    fn new(tokens: Vec<BooleanToken>) -> Self {
        Self {
            tokens,

            position: 0,

            case_sensitive: false,
        }
    }

    fn has_remaining_tokens(&self) -> bool {
        self.position < self.tokens.len()
    }

    fn peek(&self) -> Option<&BooleanToken> {
        self.tokens.get(self.position)
    }

    fn advance(&mut self) -> Option<BooleanToken> {
        let token = self.tokens.get(self.position).cloned()?;

        self.position = self.position.saturating_add(1);

        Some(token)
    }

    /*
     * Lowest precedence:
     *
     *     left OR right
     */
    fn parse_or_expression(&mut self) -> Option<BooleanExpression> {
        let mut expression = self.parse_and_expression()?;

        while matches!(self.peek(), Some(BooleanToken::Or)) {
            self.advance();

            let right = self.parse_and_expression()?;

            expression = BooleanExpression::Or(Box::new(expression), Box::new(right));
        }

        Some(expression)
    }

    /*
     * Middle precedence:
     *
     *     left AND right
     */
    fn parse_and_expression(&mut self) -> Option<BooleanExpression> {
        let mut expression = self.parse_unary_expression()?;

        while matches!(self.peek(), Some(BooleanToken::And)) {
            self.advance();

            let right = self.parse_unary_expression()?;

            expression = BooleanExpression::And(Box::new(expression), Box::new(right));
        }

        Some(expression)
    }

    /*
     * Highest precedence:
     *
     *     NOT expression
     */
    fn parse_unary_expression(&mut self) -> Option<BooleanExpression> {
        if matches!(self.peek(), Some(BooleanToken::Not)) {
            self.advance();

            let expression = self.parse_unary_expression()?;

            return Some(BooleanExpression::Not(Box::new(expression)));
        }

        self.parse_primary_expression()
    }

    fn parse_primary_expression(&mut self) -> Option<BooleanExpression> {
        /*
         * type:sensitive is a state-changing directive, not an expression.
         *
         * It applies to the next textual operand and every textual operand after
         * it for the remainder of the query.
         */
        while matches!(self.peek(), Some(BooleanToken::Sensitive)) {
            self.advance();

            self.case_sensitive = true;
        }

        match self.advance()? {
            BooleanToken::LeftParenthesis => {
                let expression = self.parse_or_expression()?;

                if !matches!(self.advance(), Some(BooleanToken::RightParenthesis)) {
                    return None;
                }

                Some(expression)
            }

            BooleanToken::Word(word) => parse_boolean_word(&word, self.case_sensitive),

            /*
             * Operators, directives, and a closing parenthesis cannot begin an
             * ordinary primary expression.
             */
            BooleanToken::And
            | BooleanToken::Or
            | BooleanToken::Not
            | BooleanToken::Sensitive
            | BooleanToken::RightParenthesis => None,
        }
    }
}

fn parse_boolean_word(word: &str, case_sensitive: bool) -> Option<BooleanExpression> {
    /*
     * Explicit classification operand:
     *
     *     type:source
     *     type:image
     */
    if let Some((prefix, value)) = word.split_once(':')
        && prefix.eq_ignore_ascii_case("type")
    {
        let normalized = value.to_lowercase();

        let filter = parse_query_type_filter(&normalized)?;

        return Some(BooleanExpression::Term(SignedQueryTerm::Type(filter)));
    }

    /*
     * Explicit extension operand:
     *
     *     ext:rs
     *     ext:.jpg
     */
    if let Some((prefix, value)) = word.split_once(':')
        && prefix.eq_ignore_ascii_case("ext")
    {
        let extension = normalize_query_extension(value)?;

        return Some(BooleanExpression::Term(SignedQueryTerm::Extension(
            extension,
        )));
    }

    /*
     * A leading plus remains a positive compact-style operand even inside a
     * Boolean expression:
     *
     *     +rs OR +cpp
     */
    if let Some(value) = word.strip_prefix('+') {
        let term = parse_signed_query_term(value, false)?;

        return Some(BooleanExpression::Term(term));
    }

    /*
     * A leading minus keeps its compact exclusion meaning:
     *
     *     rs AND -test
     *
     * It is represented as Boolean NOT so evaluation remains explicit.
     */
    if let Some(value) = word.strip_prefix('-') {
        let term = parse_signed_query_term(value, case_sensitive)?;

        return Some(BooleanExpression::Not(Box::new(BooleanExpression::Term(
            term,
        ))));
    }

    /*
     * Bare known aliases become classification or extension operands:
     *
     *     rs
     *     cpp
     *     image
     *
     * Every other word becomes ordinary path text.
     */
    let term = parse_signed_query_term(word, case_sensitive)?;

    Some(BooleanExpression::Term(term))
}

fn parse_compact_query(query: &str) -> ParsedQuery {
    let mut text_terms = Vec::new();

    let mut type_filter = None;

    let mut extension_filter = None;

    let mut include_terms = Vec::new();

    let mut exclude_terms = Vec::new();

    /*
     * Ordinary search is insensitive until type:sensitive appears.
     *
     * There is deliberately no type:insensitive directive.
     */
    let mut case_sensitive = false;

    /*
     * Search is live. Every complete recognized token is active immediately.
     *
     * Incomplete forms such as `type:` or `ext:` remain harmless until they gain
     * a valid value.
     */
    let tokens: Vec<&str> = query.split_whitespace().collect();

    let mut index = 0_usize;

    while index < tokens.len() {
        let token = tokens[index];

        if token.eq_ignore_ascii_case("type:sensitive") {
            case_sensitive = true;

            index += 1;

            continue;
        }

        /*
         * Compact type form:
         *
         *     type:python
         */
        if let Some(value) = token.strip_prefix("type:") {
            if !value.is_empty() {
                if let Some(filter) = parse_query_type_filter(&value.to_lowercase()) {
                    type_filter = Some(filter);

                    index += 1;

                    continue;
                }
            } else if let Some(next_token) = tokens.get(index + 1) {
                if let Some(filter) = parse_query_type_filter(&next_token.to_lowercase()) {
                    type_filter = Some(filter);

                    index += 2;

                    continue;
                }
            }

            if value.is_empty() {
                index += 1;

                continue;
            }
        }

        /*
         * Compact extension form:
         *
         *     ext:rs
         *     ext:.rs
         */
        if let Some(value) = token.strip_prefix("ext:") {
            if let Some(extension) = normalize_query_extension(value) {
                extension_filter = Some(extension);

                index += 1;

                continue;
            }

            if value.is_empty() {
                /*
                 * Spaced extension form:
                 *
                 *     ext: rs
                 *     ext: .rs
                 */
                if let Some(next_token) = tokens.get(index + 1) {
                    if let Some(extension) = normalize_query_extension(next_token) {
                        extension_filter = Some(extension);

                        index += 2;

                        continue;
                    }
                }
            }

            if value.is_empty() {
                index += 1;

                continue;
            }
        }

        /*
         * Unified signed modifiers:
         *
         *     +python
         *     +jpg
         *     +index
         *
         *     -java
         *     -png
         *     -.cache
         */
        if let Some(value) = token.strip_prefix('+') {
            if let Some(term) = parse_signed_query_term(value, case_sensitive) {
                include_terms.push(term);

                index += 1;

                continue;
            }

            /*
             * Spaced positive form:
             *
             *     + py
             *     + lua
             *     + .jpg
             */
            if value.is_empty() {
                if let Some(next_token) = tokens.get(index + 1) {
                    if let Some(term) = parse_signed_query_term(next_token, case_sensitive) {
                        include_terms.push(term);

                        index += 2;

                        continue;
                    }
                }

                /*
                 * An unfinished standalone '+' is harmless during live typing.
                 */
                index += 1;

                continue;
            }
        }

        if let Some(value) = token.strip_prefix('-') {
            if let Some(term) = parse_signed_query_term(value, case_sensitive) {
                exclude_terms.push(term);

                index += 1;

                continue;
            }

            /*
             * Spaced negative form:
             *
             *     - java
             *     - .cache
             */
            if value.is_empty() {
                if let Some(next_token) = tokens.get(index + 1) {
                    if let Some(term) = parse_signed_query_term(next_token, case_sensitive) {
                        exclude_terms.push(term);

                        index += 2;

                        continue;
                    }
                }

                /*
                 * An unfinished standalone '-' is harmless during live typing.
                 */
                index += 1;

                continue;
            }
        }

        /*
         * Unrecognized or ordinary tokens remain part of the free-text query.
         *
         * This prevents malformed modifiers from silently disappearing.
         */
        if case_sensitive {
            /*
             * Sensitive ordinary text becomes an explicit text condition so it can
             * carry its case-sensitive state independently from the earlier folded
             * free-text query.
             */
            include_terms.push(SignedQueryTerm::Text {
                value: token.to_string(),

                case_sensitive: true,
            });
        } else {
            text_terms.push(token);
        }

        index += 1;
    }

    ParsedQuery {
        text: text_terms.join(" ").to_lowercase(),

        type_filter,

        extension_filter,

        include_terms,

        exclude_terms,

        boolean_expression: None,
    }
}

fn normalize_query_extension(value: &str) -> Option<String> {
    let extension = value.trim().trim_start_matches('.').to_lowercase();

    if extension.is_empty() {
        None
    } else {
        Some(extension)
    }
}

fn parse_signed_query_term(value: &str, case_sensitive: bool) -> Option<SignedQueryTerm> {
    let trimmed = value.trim();

    if trimmed.is_empty() {
        return None;
    }

    let normalized = trimmed.to_lowercase();

    if normalized.is_empty() {
        return None;
    }

    /*
     * Type and language aliases win over extension aliases.
     *
     * Therefore:
     *
     *     +python
     *     -java
     *     +cpp
     *
     * are classification filters.
     */
    if let Some(filter) = parse_query_type_filter(&normalized) {
        return Some(SignedQueryTerm::Type(filter));
    }

    /*
     * A known extension becomes an extension filter.
     *
     * Both forms are accepted:
     *
     *     +jpg
     *     +.jpg
     *
     * A value such as ".cache" remains text because "cache"
     * is not a recognized file extension.
     */
    if let Some(extension) = normalize_query_extension(&normalized) {
        if classify_extension(&extension).is_some() {
            return Some(SignedQueryTerm::Extension(extension));
        }
    }

    /*

    * Everything else is ordinary filename/path text.
        */
    Some(SignedQueryTerm::Text {
        value: if case_sensitive {
            trimmed.to_string()
        } else {
            normalized
        },

        case_sensitive,
    })
}

pub(crate) fn parse_query_type_filter(value: &str) -> Option<QueryTypeFilter> {
    QUERY_TYPE_REFERENCES
        .iter()
        .find(|reference| reference.canonical == value || reference.aliases.contains(&value))
        .map(|reference| reference.filter)
}

fn entry_matches_type_filter(entry: &FileEntry, filter: QueryTypeFilter) -> bool {
    match filter {
        QueryTypeFilter::File => !entry.is_directory && !entry.is_symlink,

        QueryTypeFilter::Directory => entry.is_directory,

        QueryTypeFilter::Symlink => entry.is_symlink,

        QueryTypeFilter::Executable => entry.class == FileClass::Executable,

        QueryTypeFilter::Source => {
            matches!(
                entry.class,
                FileClass::ShellScript
                    | FileClass::Rust
                    | FileClass::Python
                    | FileClass::C
                    | FileClass::Cpp
                    | FileClass::Java
                    | FileClass::Kotlin
                    | FileClass::JavaScript
                    | FileClass::TypeScript
                    | FileClass::Assembly
                    | FileClass::Lua
                    | FileClass::Ruby
                    | FileClass::Perl
                    | FileClass::Php
                    | FileClass::Go
                    | FileClass::Swift
                    | FileClass::Dart
                    | FileClass::CSharp
                    | FileClass::Scala
                    | FileClass::Groovy
                    | FileClass::R
                    | FileClass::Awk
                    | FileClass::Elixir
                    | FileClass::Erlang
                    | FileClass::FSharp
                    | FileClass::VisualBasic
                    | FileClass::Clojure
                    | FileClass::Zig
                    | FileClass::Nim
                    | FileClass::Crystal
                    | FileClass::Haskell
                    | FileClass::Ocaml
                    | FileClass::Pascal
                    | FileClass::Solidity
                    | FileClass::Vala
                    | FileClass::Web
                    | FileClass::SourceCode
                    | FileClass::Build
            )
        }

        QueryTypeFilter::Shell => entry.class == FileClass::ShellScript,

        QueryTypeFilter::Rust => entry.class == FileClass::Rust,

        QueryTypeFilter::Python => entry.class == FileClass::Python,

        QueryTypeFilter::C => entry.class == FileClass::C,

        QueryTypeFilter::Cpp => {
            entry.class == FileClass::Cpp
                || (entry.class == FileClass::C
                    && entry
                        .path
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("h")))
        }

        QueryTypeFilter::Java => entry.class == FileClass::Java,

        QueryTypeFilter::Kotlin => entry.class == FileClass::Kotlin,

        QueryTypeFilter::JavaScript => entry.class == FileClass::JavaScript,

        QueryTypeFilter::TypeScript => entry.class == FileClass::TypeScript,

        QueryTypeFilter::Assembly => entry.class == FileClass::Assembly,

        QueryTypeFilter::Lua => entry.class == FileClass::Lua,

        QueryTypeFilter::Ruby => entry.class == FileClass::Ruby,

        QueryTypeFilter::Perl => entry.class == FileClass::Perl,

        QueryTypeFilter::Php => entry.class == FileClass::Php,

        QueryTypeFilter::Go => entry.class == FileClass::Go,

        QueryTypeFilter::Swift => entry.class == FileClass::Swift,

        QueryTypeFilter::Dart => entry.class == FileClass::Dart,

        QueryTypeFilter::CSharp => entry.class == FileClass::CSharp,

        QueryTypeFilter::Scala => entry.class == FileClass::Scala,

        QueryTypeFilter::Groovy => entry.class == FileClass::Groovy,

        QueryTypeFilter::R => entry.class == FileClass::R,

        QueryTypeFilter::Awk => entry.class == FileClass::Awk,

        QueryTypeFilter::Elixir => entry.class == FileClass::Elixir,

        QueryTypeFilter::Erlang => entry.class == FileClass::Erlang,

        QueryTypeFilter::FSharp => entry.class == FileClass::FSharp,

        QueryTypeFilter::VisualBasic => entry.class == FileClass::VisualBasic,

        QueryTypeFilter::Clojure => entry.class == FileClass::Clojure,

        QueryTypeFilter::Zig => entry.class == FileClass::Zig,

        QueryTypeFilter::Nim => entry.class == FileClass::Nim,

        QueryTypeFilter::Crystal => entry.class == FileClass::Crystal,

        QueryTypeFilter::Haskell => entry.class == FileClass::Haskell,

        QueryTypeFilter::Ocaml => entry.class == FileClass::Ocaml,

        QueryTypeFilter::Pascal => entry.class == FileClass::Pascal,

        QueryTypeFilter::Solidity => entry.class == FileClass::Solidity,

        QueryTypeFilter::Vala => entry.class == FileClass::Vala,

        QueryTypeFilter::Web => entry.class == FileClass::Web,

        QueryTypeFilter::Build => entry.class == FileClass::Build,

        QueryTypeFilter::Config => entry.class == FileClass::Config,

        QueryTypeFilter::Data => entry.class == FileClass::StructuredData,

        QueryTypeFilter::Log => entry.class == FileClass::Log,

        QueryTypeFilter::Archive => {
            matches!(entry.class, FileClass::Archive | FileClass::Package)
        }

        QueryTypeFilter::Package => entry.class == FileClass::Package,

        QueryTypeFilter::Document => {
            matches!(
                entry.class,
                FileClass::Document | FileClass::Spreadsheet | FileClass::Presentation
            )
        }

        QueryTypeFilter::Spreadsheet => entry.class == FileClass::Spreadsheet,

        QueryTypeFilter::Presentation => entry.class == FileClass::Presentation,

        QueryTypeFilter::Image => {
            matches!(entry.class, FileClass::Image | FileClass::VectorImage)
        }

        QueryTypeFilter::VectorImage => entry.class == FileClass::VectorImage,

        QueryTypeFilter::Audio => entry.class == FileClass::Audio,

        QueryTypeFilter::Video => entry.class == FileClass::Video,

        QueryTypeFilter::Font => entry.class == FileClass::Font,

        QueryTypeFilter::Database => entry.class == FileClass::Database,

        QueryTypeFilter::Torrent => entry.class == FileClass::Torrent,

        QueryTypeFilter::DesktopEntry => entry.class == FileClass::DesktopEntry,

        QueryTypeFilter::Backup => entry.class == FileClass::Backup,

        QueryTypeFilter::Certificate => entry.class == FileClass::Certificate,

        QueryTypeFilter::DiskImage => entry.class == FileClass::DiskImage,

        QueryTypeFilter::Plugin => entry.class == FileClass::Plugin,

        QueryTypeFilter::Text => entry.class == FileClass::Text,

        QueryTypeFilter::Binary => entry.class == FileClass::Binary,

        QueryTypeFilter::Unknown => entry.class == FileClass::Unknown,
    }
}

fn record_matches_type_filter(record: &SearchRecord, filter: QueryTypeFilter) -> bool {
    match filter {
        QueryTypeFilter::File => !record.is_directory && !record.is_symlink,

        QueryTypeFilter::Directory => record.is_directory,

        QueryTypeFilter::Symlink => record.is_symlink,

        QueryTypeFilter::Executable => record.class == FileClass::Executable,

        QueryTypeFilter::Source => {
            matches!(
                record.class,
                FileClass::ShellScript
                    | FileClass::Rust
                    | FileClass::Python
                    | FileClass::C
                    | FileClass::Cpp
                    | FileClass::Java
                    | FileClass::Kotlin
                    | FileClass::JavaScript
                    | FileClass::TypeScript
                    | FileClass::Assembly
                    | FileClass::Lua
                    | FileClass::Ruby
                    | FileClass::Perl
                    | FileClass::Php
                    | FileClass::Go
                    | FileClass::Swift
                    | FileClass::Dart
                    | FileClass::CSharp
                    | FileClass::Scala
                    | FileClass::Groovy
                    | FileClass::R
                    | FileClass::Awk
                    | FileClass::Elixir
                    | FileClass::Erlang
                    | FileClass::FSharp
                    | FileClass::VisualBasic
                    | FileClass::Clojure
                    | FileClass::Zig
                    | FileClass::Nim
                    | FileClass::Crystal
                    | FileClass::Haskell
                    | FileClass::Ocaml
                    | FileClass::Pascal
                    | FileClass::Solidity
                    | FileClass::Vala
                    | FileClass::Web
                    | FileClass::SourceCode
            )
        }

        QueryTypeFilter::Shell => record.class == FileClass::ShellScript,

        QueryTypeFilter::Rust => record.class == FileClass::Rust,

        QueryTypeFilter::Python => record.class == FileClass::Python,

        QueryTypeFilter::C => record.class == FileClass::C,

        QueryTypeFilter::Cpp => record.class == FileClass::Cpp,

        QueryTypeFilter::Java => record.class == FileClass::Java,

        QueryTypeFilter::Kotlin => record.class == FileClass::Kotlin,

        QueryTypeFilter::JavaScript => record.class == FileClass::JavaScript,

        QueryTypeFilter::TypeScript => record.class == FileClass::TypeScript,

        QueryTypeFilter::Assembly => record.class == FileClass::Assembly,

        QueryTypeFilter::Lua => record.class == FileClass::Lua,

        QueryTypeFilter::Ruby => record.class == FileClass::Ruby,

        QueryTypeFilter::Perl => record.class == FileClass::Perl,

        QueryTypeFilter::Php => record.class == FileClass::Php,

        QueryTypeFilter::Go => record.class == FileClass::Go,

        QueryTypeFilter::Swift => record.class == FileClass::Swift,

        QueryTypeFilter::Dart => record.class == FileClass::Dart,

        QueryTypeFilter::CSharp => record.class == FileClass::CSharp,

        QueryTypeFilter::Scala => record.class == FileClass::Scala,

        QueryTypeFilter::Groovy => record.class == FileClass::Groovy,

        QueryTypeFilter::R => record.class == FileClass::R,

        QueryTypeFilter::Awk => record.class == FileClass::Awk,

        QueryTypeFilter::Elixir => record.class == FileClass::Elixir,

        QueryTypeFilter::Erlang => record.class == FileClass::Erlang,

        QueryTypeFilter::FSharp => record.class == FileClass::FSharp,

        QueryTypeFilter::VisualBasic => record.class == FileClass::VisualBasic,

        QueryTypeFilter::Clojure => record.class == FileClass::Clojure,

        QueryTypeFilter::Zig => record.class == FileClass::Zig,

        QueryTypeFilter::Nim => record.class == FileClass::Nim,

        QueryTypeFilter::Crystal => record.class == FileClass::Crystal,

        QueryTypeFilter::Haskell => record.class == FileClass::Haskell,

        QueryTypeFilter::Ocaml => record.class == FileClass::Ocaml,

        QueryTypeFilter::Pascal => record.class == FileClass::Pascal,

        QueryTypeFilter::Solidity => record.class == FileClass::Solidity,

        QueryTypeFilter::Vala => record.class == FileClass::Vala,

        QueryTypeFilter::Web => record.class == FileClass::Web,

        QueryTypeFilter::Build => record.class == FileClass::Build,

        QueryTypeFilter::Config => record.class == FileClass::Config,

        QueryTypeFilter::Data => record.class == FileClass::StructuredData,

        QueryTypeFilter::Log => record.class == FileClass::Log,

        QueryTypeFilter::Archive => record.class == FileClass::Archive,

        QueryTypeFilter::Package => record.class == FileClass::Package,

        QueryTypeFilter::Document => record.class == FileClass::Document,

        QueryTypeFilter::Spreadsheet => record.class == FileClass::Spreadsheet,

        QueryTypeFilter::Presentation => record.class == FileClass::Presentation,

        QueryTypeFilter::Image => record.class == FileClass::Image,

        QueryTypeFilter::VectorImage => record.class == FileClass::VectorImage,

        QueryTypeFilter::Audio => record.class == FileClass::Audio,

        QueryTypeFilter::Video => record.class == FileClass::Video,

        QueryTypeFilter::Font => record.class == FileClass::Font,

        QueryTypeFilter::Database => record.class == FileClass::Database,

        QueryTypeFilter::Torrent => record.class == FileClass::Torrent,

        QueryTypeFilter::DesktopEntry => record.class == FileClass::DesktopEntry,

        QueryTypeFilter::Backup => record.class == FileClass::Backup,

        QueryTypeFilter::Certificate => record.class == FileClass::Certificate,

        QueryTypeFilter::DiskImage => record.class == FileClass::DiskImage,

        QueryTypeFilter::Plugin => record.class == FileClass::Plugin,

        QueryTypeFilter::Text => record.class == FileClass::Text,

        QueryTypeFilter::Binary => record.class == FileClass::Binary,

        QueryTypeFilter::Unknown => record.class == FileClass::Unknown,
    }
}

fn entry_matches_extension_filter(entry: &FileEntry, extension_filter: &str) -> bool {
    /*
     * Directories have no file extension for query purposes.
     */
    if entry.is_directory {
        return false;
    }

    entry
        .path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(extension_filter))
}

fn record_matches_extension_filter(record: &SearchRecord, extension_filter: &str) -> bool {
    !record.is_directory
        && record
            .extension
            .as_ref()
            .eq_ignore_ascii_case(extension_filter)
}

fn entry_matches_signed_query_term(entry: &FileEntry, term: &SignedQueryTerm) -> bool {
    match term {
        SignedQueryTerm::Type(filter) => entry_matches_type_filter(entry, *filter),

        SignedQueryTerm::Extension(extension) => entry_matches_extension_filter(entry, extension),

        SignedQueryTerm::Text {
            value,
            case_sensitive,
        } => {
            if *case_sensitive {
                entry.relative_path.to_string_lossy().contains(value)
            } else {
                entry.searchable_path.contains(value)
            }
        }
    }
}

fn entry_matches_boolean_expression(entry: &FileEntry, expression: &BooleanExpression) -> bool {
    match expression {
        BooleanExpression::Term(term) => entry_matches_signed_query_term(entry, term),

        BooleanExpression::Not(expression) => !entry_matches_boolean_expression(entry, expression),

        BooleanExpression::And(left, right) => {
            entry_matches_boolean_expression(entry, left)
                && entry_matches_boolean_expression(entry, right)
        }

        BooleanExpression::Or(left, right) => {
            entry_matches_boolean_expression(entry, left)
                || entry_matches_boolean_expression(entry, right)
        }
    }
}

fn entry_matches_positive_terms(entry: &FileEntry, terms: &[SignedQueryTerm]) -> bool {
    /*
     * Positive type and extension selectors form one OR group:
     *
     *     +lua +py
     *
     * means Lua OR Python.
     *
     * Positive ordinary text terms remain independently mandatory:
     *
     *     +index +test
     *
     * means the path must contain both "index" and "test".
     */
    let mut has_selector = false;

    let mut selector_matched = false;

    for term in terms {
        match term {
            SignedQueryTerm::Type(_) | SignedQueryTerm::Extension(_) => {
                has_selector = true;

                if entry_matches_signed_query_term(entry, term) {
                    selector_matched = true;
                }
            }

            SignedQueryTerm::Text { .. } => {
                if !entry_matches_signed_query_term(entry, term) {
                    return false;
                }
            }
        }
    }

    !has_selector || selector_matched
}

fn record_matches_signed_query_term(record: &SearchRecord, term: &SignedQueryTerm) -> bool {
    match term {
        SignedQueryTerm::Type(filter) => record_matches_type_filter(record, *filter),

        SignedQueryTerm::Extension(extension) => record_matches_extension_filter(record, extension),

        SignedQueryTerm::Text {
            value,
            case_sensitive,
        } => {
            if *case_sensitive {
                record.original_path.contains(value)
            } else {
                record.searchable_path.contains(value)
            }
        }
    }
}

fn record_matches_boolean_expression(
    record: &SearchRecord,
    expression: &BooleanExpression,
) -> bool {
    match expression {
        BooleanExpression::Term(term) => record_matches_signed_query_term(record, term),

        BooleanExpression::Not(expression) => {
            !record_matches_boolean_expression(record, expression)
        }

        BooleanExpression::And(left, right) => {
            record_matches_boolean_expression(record, left)
                && record_matches_boolean_expression(record, right)
        }

        BooleanExpression::Or(left, right) => {
            record_matches_boolean_expression(record, left)
                || record_matches_boolean_expression(record, right)
        }
    }
}

fn record_matches_positive_terms(record: &SearchRecord, terms: &[SignedQueryTerm]) -> bool {
    /*
     * Keep worker-side semantics identical to FileEntry-side semantics.
     *
     * Type and extension selectors are alternatives, while ordinary positive
     * text terms remain cumulative requirements.
     */
    let mut has_selector = false;

    let mut selector_matched = false;

    for term in terms {
        match term {
            SignedQueryTerm::Type(_) | SignedQueryTerm::Extension(_) => {
                has_selector = true;

                if record_matches_signed_query_term(record, term) {
                    selector_matched = true;
                }
            }

            SignedQueryTerm::Text { .. } => {
                if !record_matches_signed_query_term(record, term) {
                    return false;
                }
            }
        }
    }

    !has_selector || selector_matched
}

fn collect_boolean_highlight_terms(
    expression: &BooleanExpression,
    terms: &mut Vec<QueryHighlightTerm>,
) {
    match expression {
        BooleanExpression::Term(SignedQueryTerm::Text {
            value,
            case_sensitive,
        }) => {
            terms.push(QueryHighlightTerm {
                value: value.clone(),

                case_sensitive: *case_sensitive,
            });
        }

        /*
         * Type and extension operands select entries structurally. Their text
         * should not be painted as though it were a literal path match.
         */
        BooleanExpression::Term(SignedQueryTerm::Type(_) | SignedQueryTerm::Extension(_)) => {}

        /*
         * Negative Boolean operands exclude matches and therefore must never
         * contribute highlight text.
         */
        BooleanExpression::Not(_) => {}

        BooleanExpression::And(left, right) | BooleanExpression::Or(left, right) => {
            collect_boolean_highlight_terms(left, terms);

            collect_boolean_highlight_terms(right, terms);
        }
    }
}

impl ParsedQuery {
    /*
     * Ordinary unsigned text is the portion used for Exact substring matching
     * or Fuzzy relevance scoring.
     *
     * Structured modifiers are evaluated separately.
     */
    pub(crate) fn search_text(&self) -> &str {
        &self.text
    }

    pub(crate) fn highlight_terms(&self) -> Vec<QueryHighlightTerm> {
        let mut terms = Vec::new();

        /*
         * Ordinary unsigned query text remains a positive visible match.
         */
        if !self.text.is_empty() && self.text != "." {
            terms.push(QueryHighlightTerm {
                value: self.text.clone(),

                case_sensitive: false,
            });
        }

        /*
         * Positive compact text modifiers:
         *
         *     +config
         *     +index
         *
         * Type and extension selectors are intentionally ignored.
         */
        for term in &self.include_terms {
            if let SignedQueryTerm::Text {
                value,
                case_sensitive,
            } = term
            {
                terms.push(QueryHighlightTerm {
                    value: value.clone(),

                    case_sensitive: *case_sensitive,
                });
            }
        }

        /*
         * Boolean AND and OR operands are positive unless they occur beneath NOT.
         */
        if let Some(expression) = &self.boolean_expression {
            collect_boolean_highlight_terms(expression, &mut terms);
        }

        /*
         * Avoid painting the same textual operand twice.
         */
        terms.dedup();

        terms
    }

    /*
     * True when parsing produced neither ordinary search text nor a valid
     * structured filter.
     *
     * Incomplete live modifiers such as:
     *
     *     type:
     *     ext:
     *
     * are therefore harmless while the user is still typing. They must not launch
     * an all-corpus recursive search.
     */
    pub(crate) fn is_effectively_empty(&self) -> bool {
        self.text.is_empty()
            && self.type_filter.is_none()
            && self.extension_filter.is_none()
            && self.include_terms.is_empty()
            && self.exclude_terms.is_empty()
            && self.boolean_expression.is_none()
    }
}

pub(crate) fn entry_matches_query_filters(entry: &FileEntry, query: &ParsedQuery) -> bool {
    if query
        .boolean_expression
        .as_ref()
        .is_some_and(|expression| !entry_matches_boolean_expression(entry, expression))
    {
        return false;
    }

    if query
        .type_filter
        .is_some_and(|filter| !entry_matches_type_filter(entry, filter))
    {
        return false;
    }

    if query
        .extension_filter
        .as_deref()
        .is_some_and(|extension| !entry_matches_extension_filter(entry, extension))
    {
        return false;
    }

    /*
     * Positive type and extension selectors are alternatives.
     *
     * Positive ordinary path-text requirements remain cumulative.
     */
    if !entry_matches_positive_terms(entry, &query.include_terms) {
        return false;
    }

    /*
     * A match against any negative term rejects the entry.
     */
    if query
        .exclude_terms
        .iter()
        .any(|term| entry_matches_signed_query_term(entry, term))
    {
        return false;
    }

    true
}

pub(crate) fn record_matches_query_filters(record: &SearchRecord, query: &ParsedQuery) -> bool {
    if query
        .boolean_expression
        .as_ref()
        .is_some_and(|expression| !record_matches_boolean_expression(record, expression))
    {
        return false;
    }

    if query
        .type_filter
        .is_some_and(|filter| !record_matches_type_filter(record, filter))
    {
        return false;
    }

    if query
        .extension_filter
        .as_deref()
        .is_some_and(|extension| !record_matches_extension_filter(record, extension))
    {
        return false;
    }

    /*
     * Positive type and extension selectors are alternatives.
     *
     * Positive ordinary path-text requirements remain cumulative.
     */
    if !record_matches_positive_terms(record, &query.include_terms) {
        return false;
    }

    /*
     * Any matching negative term rejects the record.
     */
    if query
        .exclude_terms
        .iter()
        .any(|term| record_matches_signed_query_term(record, term))
    {
        return false;
    }

    true
}

pub(crate) fn entry_matches_query(entry: &FileEntry, query: &ParsedQuery) -> bool {
    if !entry_matches_query_filters(entry, query) {
        return false;
    }

    /*
     * Unsigned text retains ordinary Exact substring semantics.
     *
     * Fuzzy mode uses this same text for relevance scoring instead.
     */
    query.text.is_empty() || query.text == "." || entry.searchable_path.contains(&query.text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_type_modifier_is_effectively_empty() {
        let query = parse_query("type:");

        assert!(query.is_effectively_empty());
    }

    #[test]
    fn incomplete_extension_modifier_is_effectively_empty() {
        let query = parse_query("ext:");

        assert!(query.is_effectively_empty());
    }

    #[test]
    fn valid_type_modifier_is_not_effectively_empty() {
        let query = parse_query("type:image");

        assert!(!query.is_effectively_empty());
    }

    #[test]
    fn ordinary_text_is_not_effectively_empty() {
        let query = parse_query("image");

        assert!(!query.is_effectively_empty());
    }

    #[test]
    fn boolean_or_expression_is_parsed() {
        let query = parse_query("rs OR cpp");

        assert!(matches!(
            query.boolean_expression,
            Some(BooleanExpression::Or(_, _)),
        ));

        assert!(query.search_text().is_empty());
    }

    #[test]
    fn boolean_operators_are_case_insensitive() {
        let query = parse_query("rs or cpp");

        assert!(matches!(
            query.boolean_expression,
            Some(BooleanExpression::Or(_, _)),
        ));
    }

    #[test]
    fn boolean_and_has_higher_precedence_than_or() {
        let query = parse_query("rs OR cpp AND test");

        let Some(BooleanExpression::Or(_, right)) = query.boolean_expression else {
            panic!("expected top-level OR expression");
        };

        assert!(matches!(*right, BooleanExpression::And(_, _),));
    }

    #[test]
    fn parentheses_override_boolean_precedence() {
        let query = parse_query("(rs OR cpp) AND test");

        assert!(matches!(
            query.boolean_expression,
            Some(BooleanExpression::And(_, _)),
        ));
    }

    #[test]
    fn incomplete_boolean_expression_is_effectively_empty() {
        let query = parse_query("rs OR");

        assert!(query.is_effectively_empty());
    }

    #[test]
    fn compact_query_does_not_become_boolean() {
        let query = parse_query("type:source +rs +cpp -test");

        assert!(query.boolean_expression.is_none());

        assert!(!query.is_effectively_empty());
    }

    #[test]
    fn compact_sensitive_directive_preserves_following_case() {
        let parsed = parse_query("before type:sensitive TeSt");

        assert_eq!(parsed.search_text(), "before");

        assert!(matches!(
            parsed.include_terms.as_slice(),
            [
                SignedQueryTerm::Text {
                    value,
                    case_sensitive: true,
                }
            ] if value == "TeSt"
        ));
    }

    #[test]
    fn compact_sensitive_directive_alone_is_effectively_empty() {
        let parsed = parse_query("type:sensitive");

        assert!(parsed.is_effectively_empty());
    }

    #[test]
    fn boolean_sensitive_directive_preserves_following_case() {
        let parsed = parse_query("rs AND type:sensitive TeSt");

        let Some(BooleanExpression::And(_, right)) = parsed.boolean_expression.as_ref() else {
            panic!("expected Boolean AND expression");
        };

        assert!(matches!(
            right.as_ref(),
            BooleanExpression::Term(
                SignedQueryTerm::Text {
                    value,
                    case_sensitive: true,
                }
            ) if value == "TeSt"
        ));
    }

    #[test]
    fn positive_compact_text_terms_are_highlighted() {
        let query = parse_query("+settings +index");

        assert_eq!(
            query.highlight_terms(),
            vec![
                QueryHighlightTerm {
                    value: "settings".to_string(),
                    case_sensitive: false,
                },
                QueryHighlightTerm {
                    value: "index".to_string(),
                    case_sensitive: false,
                },
            ],
        );
    }

    #[test]
    fn negative_compact_terms_are_not_highlighted() {
        let query = parse_query("index -java");

        assert_eq!(
            query.highlight_terms(),
            vec![QueryHighlightTerm {
                value: "index".to_string(),
                case_sensitive: false,
            }],
        );
    }

    #[test]
    fn boolean_and_and_or_text_operands_are_highlighted() {
        let query = parse_query("(settings OR index) AND testing");

        assert_eq!(
            query.highlight_terms(),
            vec![
                QueryHighlightTerm {
                    value: "settings".to_string(),
                    case_sensitive: false,
                },
                QueryHighlightTerm {
                    value: "index".to_string(),
                    case_sensitive: false,
                },
                QueryHighlightTerm {
                    value: "testing".to_string(),
                    case_sensitive: false,
                },
            ],
        );
    }

    #[test]
    fn boolean_not_operand_is_not_highlighted() {
        let query = parse_query("settings AND NOT generated");

        assert_eq!(
            query.highlight_terms(),
            vec![QueryHighlightTerm {
                value: "settings".to_string(),
                case_sensitive: false,
            }],
        );
    }

    #[test]
    fn sensitive_positive_term_retains_exact_case_for_highlighting() {
        let query = parse_query("type:sensitive README");

        assert_eq!(
            query.highlight_terms(),
            vec![QueryHighlightTerm {
                value: "README".to_string(),
                case_sensitive: true,
            }],
        );
    }

    #[test]
    fn every_documented_query_type_name_is_parseable() {
        for reference in QUERY_TYPE_REFERENCES {
            assert_eq!(
                parse_query_type_filter(reference.canonical),
                Some(reference.filter),
                "canonical query type failed to parse: {}",
                reference.canonical,
            );

            for alias in reference.aliases {
                assert_eq!(
                    parse_query_type_filter(alias),
                    Some(reference.filter),
                    "query type alias failed to parse: {}",
                    alias,
                );
            }
        }
    }

    #[test]
    fn documented_query_type_names_are_unique() {
        let mut names = std::collections::HashSet::new();

        for reference in QUERY_TYPE_REFERENCES {
            assert!(
                names.insert(reference.canonical),
                "duplicate canonical query type: {}",
                reference.canonical,
            );

            for alias in reference.aliases {
                assert!(
                    names.insert(*alias),
                    "duplicate query type alias: {}",
                    alias,
                );
            }
        }
    }
}
