# The README's recording: lazyrunner on the demo shop (examples/demo) - trusting its profile, the startup checklist,
# building and starting everything, then the requests view. VHS (https://github.com/charmbracelet/vhs) records it;
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
Sleep 1s

# The command palette.
Type ":"
Sleep 1s
Type "test"
Sleep 2s
Escape
Sleep 2s
