# Config helpers. AGENTLOOP_HOME must be set by the caller.
config_to_json() { # yaml_path -> json on stdout
  python3 "$AGENTLOOP_HOME/helpers/yaml2json.py" "$1"
}

# Echo a role's field, or empty if absent.
config_role_field() { # config_json role field
  printf '%s' "$1" | jq -r --arg r "$2" --arg f "$3" '.routing[$r][$f] // empty'
}

# Echo the role to actually use: the role if present in routing, else defaults.role.
# Single jq pass; `// empty` yields nothing (not the string "null") if defaults.role is absent.
config_resolve_role() { # config_json role
  printf '%s' "$1" | jq -r --arg r "$2" 'if .routing | has($r) then $r else .defaults.role // empty end'
}

config_cap() { # config_json cap_key
  printf '%s' "$1" | jq -r --arg k "$2" '.caps[$k] // empty'
}
