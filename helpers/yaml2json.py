#!/usr/bin/env python3
import sys, json, yaml
with open(sys.argv[1]) as f:
    json.dump(yaml.safe_load(f), sys.stdout)
