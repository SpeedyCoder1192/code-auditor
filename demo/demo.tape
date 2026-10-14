# Renders demo/demo.gif. Needs vhs, ttyd, ffmpeg and a running Ollama.
#   cargo build --release && vhs demo/demo.tape

Output demo/demo.gif
Require ollama

Set Shell "bash"
Set FontSize 15
Set Width 1200
Set Height 640
Set Padding 16
Set TypingSpeed 50ms
Set Theme "Catppuccin Mocha"

Hide
Type "export PATH=$PWD/target/release:$PATH PS1='$ ' && cd demo && clear"
Enter
Show

Type "auditor scan sample/"
Sleep 500ms
Enter
Sleep 3s
# Cut most of the model wait (about 45 s on a CPU) out of the GIF.
Hide
Sleep 55s
Show
Sleep 3s
Type "n"
Sleep 2s
Type "n"
Sleep 2s
Tab
Sleep 2s
Type "q"
Sleep 1s
