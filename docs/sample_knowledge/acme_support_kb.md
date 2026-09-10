# Acme Wi-Fi Mesh Router — Support Knowledge Base

## Wi-Fi Router — ACME-M6

### Setup
The ACME-M6 boots in about 40 seconds. To set it up, plug the power adapter
into a wall outlet, wait for the LED ring to turn solid white, then connect to
the network `ACME-M6-Setup` from your phone or laptop. The setup wizard opens
automatically at `http://192.168.10.1`. Default admin credentials are printed
on the label on the bottom of the unit. Change the admin password during setup;
the device refuses cloud remote management until the default password is
changed.

### LED meanings
- Solid white: normal operation.
- Pulsing amber: firmware update in progress; do not unplug.
- Solid red: no internet on the WAN port. Check the Ethernet cable from your
  modem to the blue WAN port.
- Blinking red: overheating. Move the router to a ventilated spot; the fan
  spins up automatically above 70 degrees Celsius.

### Wi-Fi drops every hour
If Wi-Fi drops every 60 minutes on the 2.4 GHz band, this is almost always the
legacy "Auto channel hop" feature. Open the admin page, go to
Wireless > Advanced, and set "Channel selection" to Manual with channel 1, 6
or 11. Firmware 2.4.1 or later disables auto-hopping by default; update via
Settings > Firmware.

### Guest network
Guest Wi-Fi is enabled in Wireless > Guest network. Guest devices are isolated
from the LAN by default and bandwidth is capped at 50 Mbit/s per guest. The
guest password rotates every 24 hours by default; you can pin it for 30 days.

### Factory reset
Hold the recessed reset button with a paperclip for 12 seconds while powered
on. The LED ring flashes red three times. All settings are erased, including
port forwards and DNS rules.

### Port forwarding
Port forwards live under Network > NAT > Port forwarding. Up to 64 rules are
supported. UPnP is disabled by default for security; enable it only if a game
console requires it.

## ACME PowerBank 20K

The ACME PowerBank 20K has a 20,000 mAh cell, 65 W USB-C PD output and a
pass-through charging mode. A full phone charge takes about 90 minutes.

### Not charging a laptop
If a laptop will not charge, make sure you use the USB-C port labelled
"OUT 65W" — the other port tops out at 18 W. Also press the power button
once; the unit sleeps after 30 seconds of inactivity to conserve charge.

### Reconditioning the battery
After roughly 50 cycles, hold the power button for 5 seconds until the LEDs
sweep twice to recalibrate the fuel gauge.

### Warranty
The PowerBank carries an 18-month limited warranty. Batteries are consumables;
capacity loss below 80 percent within the warranty window qualifies for a
replacement when a diagnostic report is attached.
