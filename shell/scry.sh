# Scry shell integration for sh, bash, ksh, and zsh.
#
# Source this file from the shell's startup file. Do not execute it.
# Set SCRY_BINARY before sourcing to use a development binary; when it is
# unset, the helper runs the `scry` binary found in PATH.

scry() {
    if [ -n "${KSH_VERSION-}" ]; then
        typeset _scry_handoff_file _scry_destination _scry_status _scry_binary
    else
        local _scry_handoff_file _scry_destination _scry_status _scry_binary
    fi

    _scry_handoff_file=$(mktemp "${TMPDIR:-/tmp}/scry-cd.XXXXXXXXXX") || {
        printf '%s\n' 'scry: could not create the shell handoff file' >&2
        return 1
    }

    _scry_binary=${SCRY_BINARY:-scry}

    SCRY_CD_FILE=$_scry_handoff_file command "$_scry_binary" "$@"
    _scry_status=$?

    if [ -s "$_scry_handoff_file" ]; then
        # The trailing dot preserves trailing newlines in valid Unix paths;
        # command substitution would otherwise remove them.
        _scry_destination=$(command cat "$_scry_handoff_file"; printf '.')
        _scry_destination=${_scry_destination%.}

        case $_scry_destination in
            /*)
                if [ -d "$_scry_destination" ]; then
                    cd "$_scry_destination" || _scry_status=$?
                else
                    printf 'scry: handoff directory no longer exists: %s\n' \
                        "$_scry_destination" >&2
                    _scry_status=1
                fi
                ;;
            *)
                printf 'scry: refused a non-absolute handoff path: %s\n' \
                    "$_scry_destination" >&2
                _scry_status=1
                ;;
        esac
    fi

    command rm -f "$_scry_handoff_file"
    return "$_scry_status"
}
