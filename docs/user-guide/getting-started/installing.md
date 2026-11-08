---
title: Installing Stalagmight
description: Download and install Stalagmight on Windows, on a Mac with Apple Silicon, or on Linux.
order: 1
tags: [install]
# An example of the conventions in ../README.md. Remove this line to publish it.
draft: true
---

Stalagmight runs on 64-bit Windows, on Macs with Apple Silicon (M1 and later),
and on 64-bit (x86-64) Linux. It is free: download the file for your system
from the [download section](https://stalagmight.com/#download) of the website,
or pick any version on the [releases page](https://stalagmight.com/releases/).

## Windows

Download the `.exe` installer, or the `.msi` package if you deploy software
with Windows Installer, and run it.

> [!NOTE]
> The Windows builds are not code-signed yet. When SmartScreen says "Windows
> protected your PC", choose **More info**, then **Run anyway**.

## macOS

Download the `.dmg` disk image and open it. Stalagmight needs a Mac with Apple
Silicon; **Intel Macs are not supported**.

## Linux

Pick the file that suits your distribution:

| File | For |
|---|---|
| AppImage | Any distribution. Make it executable with `chmod +x`, then run it. |
| `.deb` | Debian, Ubuntu and their derivatives. Install it with `apt`. |
| `.rpm` | Fedora and openSUSE. Install it with `dnf` or `zypper`. |

## Checking the download

Every release lists the SHA-256 checksum of each file in its `SHA256SUMS`
file, so you can confirm a download arrived intact.
