# The README's recording: lazyrunner on the demo shop (examples/demo) - trusting its profile, the startup checklist,
# building and starting everything; then the stack map with the shopper's calls moving, a fault set on them and the
# 500s it brings, a search, the Problems screen, the code map and the Health tab, the timeline, the requests view and
# a request's details, the durations and the command palette. VHS (https://github.com/charmbracelet/vhs) records it;
# .github/workflows/demo.yml runs it. Locally, with vhs, dotnet, node and lazyrunner on PATH: vhs docs/demo.tape

Output docs/demo.gif

Require lazyrunner
Require dotnet
Require node

Set Shell "bash"
Set FontSize 15
Set Width 1600
Set Height 900
Set Padding 18
Set Theme "Catppuccin Mocha"
Set WindowBar Colorful
Set TypingSpeed 70ms
Set Framerate 24

Env LAZYRUNNER_CONFIG_PATH "/tmp/lazyrunner-demo/config.json"
Env LAZYRUNNER_STATE_DIR "/tmp/lazyrunner-demo/state"

Hide
Type `mkdir -p /tmp/lazyrunner-demo && echo '{"theme":"catppuccin-mocha","nerdIcons":false,"autoStartOnOpen":true}' > /tmp/lazyrunner-demo/config.json`
Enter
# The demo as a project of its own - its own folder and git repo, on main - as it would be on your machine.
Type `cp -r examples/demo ~/demo-shop && cd ~/demo-shop && git init -q -b main && git add -A && git -c user.name=demo -c user.email=demo@example.com commit -qm "Demo shop" && clear`
Enter
Show

Sleep 800ms
Type "lazyrunner"
Sleep 500ms
Enter
Wait+Screen@15s /Trust it and run these/
Sleep 1.5s
Type "y"
Sleep 300ms
Enter

# The checklist, the builds and the starts, as fast as they really are.
Wait+Screen@180s /Ready in/
Sleep 4s

# The stack map, on the shop (the third app): its calls to Orders and Billing travel along the edges.
Type "3"
Sleep 400ms
Type "K"
Sleep 3.5s

# A fault on the shop's calls to Orders: the chaos preset - for 30 s, half wait up to 2 s and 3 in 10 fail with 500.
Type "f"
Sleep 1.5s
Up
Sleep 500ms
Right
Sleep 350ms
Right
Sleep 350ms
Right
Sleep 350ms
Right
Sleep 1.5s
Enter
Sleep 1s
Escape
Sleep 3.5s
Escape

# Back on the columns: the shopper's calls now come back 500, some of them slow.
Sleep 4s

# A search in the shop's output, filtered to its matches.
Type "/"
Sleep 300ms
Type "500"
Sleep 800ms
Tab
Sleep 2.5s
Escape
Sleep 800ms

# Every app's errors and warnings, grouped.
Type "E"
Sleep 3.5s
Escape
Sleep 500ms

# The code map and the Health tab, beside the map.
Type "K"
Sleep 1.5s
Tab
Sleep 3s
Tab
Sleep 3.5s
Escape
Sleep 500ms

# Every app's output in the order it came.
Type "~"
Sleep 3s
Escape
Sleep 500ms

# The requests the shop's shopper made, and one request's details.
Type "H"
Sleep 2.5s
Type "j"
Sleep 400ms
Type "j"
Sleep 400ms
Type "j"
Sleep 600ms
Enter
Sleep 4s
Escape
Sleep 300ms
Escape
Sleep 800ms

# How long the builds and starts took.
Type "D"
Sleep 3s
Escape
Sleep 500ms

# The command palette.
Type ":"
Sleep 1s
Type "test"
Sleep 2s
Escape
Sleep 2s
