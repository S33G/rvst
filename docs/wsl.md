# RVST in WSL

The setup wizard detects WSL and asks before installing RVST inside the Linux
distribution. That installs Linux command-line binaries in Cargo's Linux
directory. It does not install a Windows executable or plugins for a Windows
DAW.

If your DAW runs in Windows, install RVST with a Windows Rust toolchain from
PowerShell and build the plugin for Windows there. If your DAW runs in Linux
inside WSL, continue with the Linux toolchain and the audio setup required by
that distribution and host.

The installer does not configure audio routing. It prints this guide whenever
it detects WSL so you can choose the environment that matches your DAW.
