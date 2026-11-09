TEST-ONLY keys. Each secret seed is sha256("hideout-test-vectors/TEST-ONLY/" + name), so anyone can
regenerate them (tools/generate.py). They protect nothing. Never pin one in a release build.
No real key is ever put in this folder.
