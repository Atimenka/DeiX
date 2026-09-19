# USB 2.0 (EHCI) Subsystem in DeiX OS

DeiX OS v0.2.1 includes native support for USB 2.0 EHCI (Enhanced Host Controller Interface).

## Supported Controllers and Classes
- **Host Controller**: EHCI PCI Class 0x0C, Subclass 0x03, ProgIF 0x20.
- **USB Core**: Full device enumeration (GET_DESCRIPTOR, SET_ADDRESS, SET_CONFIGURATION).
- **USB Hub Class (0x09)**: Root Hub and external hub port status polling.
- **USB HID Class (0x03)**: Keyboards and Mice input processing.
- **USB Mass Storage (0x08)**: Bulk-Only Transport (BOT) and SCSI READ/WRITE (10) commands.

## CLI Commands
- `usb info`: Shows EHCI controller status and schedules.
- `usb devices`: Lists connected USB devices with VID/PID and Address.
- `usb tree`: Displays USB bus device hierarchy.
