#!/bin/bash
# DeiX OS — Full hardware emulation + networking
DISK="$(dirname "$0")/build/deix_disk.img"

echo "=== DeiX OS v0.2 ==="
echo "512 MB RAM | RTL8139 NIC | 10.0.2.15 IP"
echo "Commands: browser, files, ping 10.0.2.2, gpu mode"
echo ""

exec qemu-system-x86_64 \
  -drive file="$DISK",format=raw \
  -m 512M \
  -net nic,model=rtl8139 \
  -net user \
  -vga std \
  -serial stdio \
  -no-reboot
