# Scry shell integration runtime for csh and tcsh.
#
# This file is sourced by the `scry` alias for each invocation. Do not source
# it directly from .cshrc; define the alias documented with this file instead.

set _scry_tmpdir = /tmp
if ($?TMPDIR) then
    if ("$TMPDIR" != "") set _scry_tmpdir = "$TMPDIR"
endif

set _scry_handoff_file = "`mktemp "$_scry_tmpdir/scry-cd.XXXXXXXXXX"`"
if ("$_scry_handoff_file" == "") then
    echo "scry: could not create the shell handoff file" > /dev/stderr
    set _scry_status = 1
    goto scry_done
endif

set _scry_binary = scry
if ($?SCRY_BINARY) then
    if ("$SCRY_BINARY" != "") set _scry_binary = "$SCRY_BINARY"
endif

env SCRY_CD_FILE="$_scry_handoff_file" "$_scry_binary" $argv:q
set _scry_status = $status

if (-s "$_scry_handoff_file") then
    set _scry_destination = "`/bin/cat "$_scry_handoff_file"`"

    switch ("$_scry_destination")
        case /*:
            if (-d "$_scry_destination") then
                cd "$_scry_destination"
                if ($status != 0) set _scry_status = $status
            else
                echo "scry: handoff directory no longer exists: $_scry_destination" > /dev/stderr
                set _scry_status = 1
            endif
            breaksw

        default:
            echo "scry: refused a non-absolute handoff path: $_scry_destination" > /dev/stderr
            set _scry_status = 1
            breaksw
    endsw
endif

scry_done:
if ($?_scry_handoff_file) then
    if ("$_scry_handoff_file" != "") /bin/rm -f "$_scry_handoff_file"
endif

# Expand the saved numeric status into the command before unsetting our
# temporary variables, then leave that status as the result of `source`.
eval "unset _scry_tmpdir _scry_handoff_file _scry_destination _scry_binary _scry_status; /bin/sh -c 'exit $_scry_status'"
