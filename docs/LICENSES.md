# Third-party licenses

Original SBM Core code is MIT OR Apache-2.0. Recovery tools are separate programs. The daemon execs them; it does not copy their source into the Rust crates.

The image build writes the RPM license field for every installed package to `/usr/share/sbm/package-licenses.tsv`. That file is the license record for a given image. A copy is placed next to the image output. Do not treat the list below as that record until the build has produced the TSV.

Packages requested by name, and the SPDX identifiers Fedora publishes for them, include:

| Package | Role | License as published by Fedora (confirm in the TSV) |
|---|---|---|
| gparted | partition UI | GPL-2.0-or-later |
| parted | partition library | GPL-3.0-or-later |
| e2fsprogs | ext4 | GPL-2.0-only AND LGPL-2.0-only AND BSD-3-Clause AND MIT |
| dosfstools | FAT | GPL-3.0-or-later |
| ntfs-3g / ntfsprogs | NTFS | GPL-2.0-or-later AND LGPL-2.0-or-later |
| exfatprogs | exFAT | GPL-2.0-only |
| cryptsetup | LUKS | GPL-2.0-or-later WITH cryptsetup-OpenSSL-exception AND LGPL-2.1-or-later |
| btrfs-progs | Btrfs | GPL-2.0-only AND LGPL-2.1-or-later |
| partclone | imaging | GPL-2.0-only |
| testdisk | TestDisk and PhotoRec | GPL-2.0-or-later |
| ddrescue | GNU ddrescue | GPL-2.0-or-later |
| smartmontools | SMART | GPL-2.0-or-later |
| nvme-cli | NVMe | GPL-2.0-only |
| efibootmgr | EFI boot entries | GPL-2.0-only |
| rsync | copy | GPL-3.0-or-later |
| openssh | SSH client and server | BSD-2-Clause AND BSD-3-Clause AND ISC AND MIT |
| wimlib-utils | WIM | LGPL-2.1-or-later AND GPL-3.0-or-later |
| labwc, wayvnc, foot | desktop and remote display | GPL / ISC / MIT, see the package |

`linux-firmware-whence` is installed so the firmware license text is on the image. Firmware files stay in their own packages and are not rewritten.
