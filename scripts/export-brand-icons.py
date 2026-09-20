#!/usr/bin/env python3
"""Export the approved D2 artwork to the existing desktop asset locations."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'apps/desktop/branding/d2-scan-ring'
PUBLIC = ROOT / 'apps/desktop/public'
ICONS = ROOT / 'apps/desktop/src-tauri/icons'


def render(source, output, size):
    subprocess.run(['rsvg-convert', '-w', str(size), '-h', str(size), '-o', str(output), str(source)], check=True)


def main():
    for command in ['rsvg-convert', 'iconutil']:
        if shutil.which(command) is None:
            raise SystemExit(f'Required export tool is missing: {command}')

    for source, output in {
        'mark.svg': 'logo.svg', 'mark-dark.svg': 'logo-dark.svg',
        'mark-small.svg': 'logo-small.svg', 'mark-small-dark.svg': 'logo-small-dark.svg',
        'favicon.svg': 'favicon.svg',
    }.items():
        shutil.copyfile(SOURCE / source, PUBLIC / output)
    shutil.copyfile(SOURCE / 'tray-template.svg', ICONS / 'tray-template.svg')

    jobs = [(SOURCE / 'app-light.svg', PUBLIC / 'icon.png', 256)]
    fixed = {'32x32.png': 32, '128x128.png': 128, '128x128@2x.png': 256,
             'icon.png': 512, 'icon-light.png': 1024, 'StoreLogo.png': 50}
    fixed.update({f'Square{n}x{n}Logo.png': n for n in [30, 44, 71, 89, 107, 142, 150, 284, 310]})
    jobs.extend((SOURCE / ('app-small.svg' if size <= 32 else 'app-light.svg'), ICONS / name, size)
                for name, size in fixed.items())
    jobs.extend([(SOURCE / 'app-dark.svg', ICONS / 'icon-dark.png', 1024),
                 (SOURCE / 'tray-template.svg', ICONS / 'tray-template.png', 36)])
    with ThreadPoolExecutor(max_workers=6) as pool:
        list(pool.map(lambda job: render(*job), jobs))

    with tempfile.TemporaryDirectory(prefix='token-station-brand-') as directory:
        staging = Path(directory)
        images = []
        for size in [16, 24, 32, 48, 64, 128, 256]:
            output = staging / f'icon-{size}.png'
            render(SOURCE / ('app-small.svg' if size <= 32 else 'app-light.svg'), output, size)
            images.append((size, output.read_bytes()))
        offset = 6 + 16 * len(images)
        entries, payload = [], []
        for size, png in images:
            encoded_size = 0 if size == 256 else size
            entries.append(struct.pack('<BBBBHHII', encoded_size, encoded_size, 0, 0, 1, 32, len(png), offset))
            payload.append(png)
            offset += len(png)
        (ICONS / 'icon.ico').write_bytes(struct.pack('<HHH', 0, 1, len(images)) + b''.join(entries) + b''.join(payload))

        iconset = staging / 'app.iconset'
        iconset.mkdir()
        for size in [16, 32, 128, 256, 512]:
            for scale in [1, 2]:
                pixels = size * scale
                output = iconset / f'icon_{size}x{size}{"@2x" if scale == 2 else ""}.png'
                render(SOURCE / ('app-small.svg' if pixels <= 32 else 'app-light.svg'), output, pixels)
        subprocess.run(['iconutil', '-c', 'icns', str(iconset), '-o', str(ICONS / 'icon.icns')], check=True)
    print('D2 desktop artwork exported.')


if __name__ == '__main__':
    main()
