# Bash completion for srm. Completion never runs a deletion or remote request.
_srm_complete() {
    local current="${COMP_WORDS[COMP_CWORD]}" previous="${COMP_WORDS[COMP_CWORD-1]}"
    local first=1 command sub config="" i options ids helper
    local -a prefix=()
    COMPREPLY=()
    # sudo delegates completion differently across bash-completion versions.
    # Never invoke sudo or inspect root's recovery records from this function.
    if [[ ${COMP_WORDS[0]##*/} = sudo ]]; then
        for ((first=1; first<COMP_CWORD; first++)); do
            [[ ${COMP_WORDS[first]##*/} = srm ]] && break
        done
        ((first++))
    fi
    while [[ ${COMP_WORDS[first]:-} = --config || ${COMP_WORDS[first]:-} = --lang || ${COMP_WORDS[first]:-} = --lang=* ]]; do
        if [[ ${COMP_WORDS[first]} = --lang=* ]]; then ((first++)); continue; fi
        if [[ ${COMP_WORDS[first]} = --lang ]]; then
            if ((COMP_CWORD == first+1)); then mapfile -t COMPREPLY < <(compgen -W 'auto zh en' -- "$current"); return; fi
            ((first+=2)); continue
        fi
        if ((COMP_CWORD == first+1)); then
            mapfile -t COMPREPLY < <(compgen -f -- "$current")
            compopt -o filenames 2>/dev/null || true
            return
        fi
        config="${COMP_WORDS[first+1]:-}"
        ((first+=2))
    done
    command="${COMP_WORDS[first]:-}"
    sub="${COMP_WORDS[first+1]:-}"
    for ((i=first; i<COMP_CWORD; i++)); do
        if [[ ${COMP_WORDS[i]} = -- ]]; then
            mapfile -t COMPREPLY < <(compgen -f -- "$current")
            compopt -o filenames 2>/dev/null || true
            return
        fi
    done
    if [[ $previous = --target || $previous = -t || $previous = --config ]]; then
        mapfile -t COMPREPLY < <(compgen -f -- "$current")
        compopt -o filenames 2>/dev/null || true
        return
    fi
    if [[ $previous = --expire-days || ( $previous = -d && ( $command = del || $command = delete ) ) ]]; then
        mapfile -t COMPREPLY < <(compgen -W '0 1 3 7 14 30 90' -- "$current")
        return
    fi
    case "$current" in
        --lang=*) mapfile -t COMPREPLY < <(compgen -W '--lang=auto --lang=zh --lang=en' -- "$current"); return ;;
        --interactive=*) mapfile -t COMPREPLY < <(compgen -W '--interactive=never --interactive=once --interactive=always' -- "$current"); return ;;
        --preserve-root=*) mapfile -t COMPREPLY < <(compgen -W '--preserve-root=all' -- "$current"); return ;;
    esac
    options='-f -i -I -r -R -d -v --force --interactive= --recursive --dir --verbose --one-file-system --preserve-root --no-preserve-root --help --version --'
    if ((COMP_CWORD == first)); then
        mapfile -t COMPREPLY < <(compgen -W "delete del restore res list ls clean cln empty doctor recover config remote help --config --lang --language-info --rm $options" -- "$current")
        [[ $current = -* ]] || { mapfile -t prefix < <(compgen -f -- "$current"); COMPREPLY+=("${prefix[@]}"); }
        compopt -o filenames 2>/dev/null || true
        return
    fi
    case "$command" in
        remote)
            if ((COMP_CWORD == first+1)); then
                mapfile -t COMPREPLY < <(compgen -W 'init status list sync retarget restore delete --help' -- "$current"); return
            fi
            case "$sub" in
                restore) options='--target -t --help' ;;
                delete|retarget) options='--yes -y --help' ;;
                *) options='--help' ;;
            esac ;;
        config) options='--init --help' ;;
        list|ls) options='--expired --verbose -v --json --measure --help' ;;
        restore|res) options='--force -f --target -t --help' ;;
        recover) options='--help' ;;
        clean|cln) options='--all -a --dry-run --help' ;;
        empty) options='--yes -y --help' ;;
        doctor) options='--repair-permissions --help' ;;
        del|delete) options='--expire-days -d -f -i -I -r -R -v --force --interactive= --recursive --verbose --one-file-system --preserve-root --no-preserve-root --help --' ;;
        help) mapfile -t COMPREPLY < <(compgen -W 'list restore clean empty doctor recover config remote' -- "$current"); return ;;
    esac
    if [[ $current = -* ]]; then
        mapfile -t COMPREPLY < <(compgen -W "$options" -- "$current"); return
    fi
    case "$command" in
        restore|res|recover)
            [[ ${COMP_WORDS[0]##*/} = sudo ]] && return
            [[ -n ${SUDO_COMMAND:-} ]] && return
            helper="${_SRM_COMPLETION_HELPER:-$HOME/.local/lib/safe-srm/completion_ids.py}"
            [[ -f $helper ]] || helper=/usr/local/share/safe-srm/completion_ids.py
            if [[ -r $helper ]] && command -v python3 >/dev/null; then
                mapfile -t COMPREPLY < <(python3 "$helper" bash "$config" "$current" 2>/dev/null)
            fi ;;
        remote|config|list|ls|clean|cln|empty|doctor) ;;
        *) mapfile -t COMPREPLY < <(compgen -f -- "$current"); compopt -o filenames 2>/dev/null || true ;;
    esac
}
complete -F _srm_complete srm
