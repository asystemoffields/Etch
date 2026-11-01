# Hardware parity with Microsoft Word

Word is not a hardware-heavy app, but a few devices matter to its users: printers above all, then
pens, microphones (Dictate) and speakers (Read Aloud), and displays.

**Dimension: ~40% (estimated).**

| Hardware | Word | Etch macOS | Windows | Linux / BSD | Web | Parity |
|---|---|---|---|---|---|---|
| Printers: native print dialog, printer choice, copies, duplex, page ranges, tray, scaling | ✅ (`WordPDE.plugin` print-dialog extension on Mac) | 🟡 Direct CUPS printing; printer, copies, collation, duplex, ranges, grayscale and scale to paper size | 🟡 Direct spooler printing; printer, copies, collation, duplex, ranges, grayscale and scale to paper size | 🟡 Direct spooler printing; printer, copies, collation, duplex, ranges, grayscale and scale to paper size | ✅ browser print dialog | 35% |
| GPU rendering, HiDPI/Retina, pixel-aligned text | ✅ | ✅ wgpu + vello_cpu rasteriser, whole-pixel text | ✅ DirectX 12 default, OpenGL when DirectX 12 has no GPU driver or on 32-bit; startup crash on Intel UHD | ✅ (soft text on KDE at some zooms) | ✅ | 80% |
| Pen / stylus: ink, pressure, eraser, ink to shape/math | ✅ (Draw tab, `InkRender.bundle`) | 🟡 ink, stroke eraser, Ink to Shape incl. one- and two-stroke arrows with open or closed heads; pressure and Ink to Math missing | 🟡 same | 🟡 same | 🟡 same | 50% |
| Multiple monitors, per-monitor DPI, remembered window position, a window that fits its screen | ✅ | ✅ | ✅ (winit DPI fix pending) | ✅ | — | 85% |
| Microphone: Dictate | ✅ (cloud speech service) | ❌ | ❌ | ❌ | ❌ | 0% |
| Speakers: Read Aloud | ✅ | ✅ system speech, speed, sentence skip | ✅ | 🟡 | 🟡 | 75% |
| Touch screens and touchpads: pinch zoom, inertial scroll | ✅ | ✅ pinch | ✅ | ✅ inertial scroll | ✅ | 85% |
| Scanners/cameras (Insert from device / Continuity Camera on Mac) | ✅ | ❌ | ❌ | ❌ | ❌ | 0% |
