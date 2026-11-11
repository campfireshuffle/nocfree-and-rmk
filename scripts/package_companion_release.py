#!/usr/bin/env python3
"""Package the reviewed firmware images without rebuilding or touching devices."""
import argparse
import hashlib
import json
from pathlib import Path
import sys

from image_guard import inspect as inspect_receiver
from migration_guard import inspect_startup_image

ROOT = Path(__file__).resolve().parents[1]
VERSION = '1.1.2'
MANIFEST_SHA256 = '4af388295967013fac3fd947e7f394f2bdedc18473e1640db0ded9f673f6ef95'
SOURCE = '.evidence/native-release-1.1.2/companion'
ROLES = {
    'left': ('native-release-1.1.2/companion', 'e97deaed01379c72554bff9bb85c5d39d12248bd624b376ad0babbc920d4ecb2', 'f3efc065e0fbaa1f042c5ab386618f841ed81939bea6f57f5176f8e0e937bce7', '7b984e911266dee07486d1c712784671a78a03ed', '434ab4d7d29d8e9ba689837358c8a44996ba38cc'),
    'right': ('native-release-1.1.2/companion', 'e411fa52d33e0f5705ba5e826f5b37704d0d4fc5007d7468c0af8bc52ed77979', 'dd4e2e7cce1397730a3c2585acee9f981f7fc8bd20027b9981fe6cb288bba71f', '7b984e911266dee07486d1c712784671a78a03ed', '434ab4d7d29d8e9ba689837358c8a44996ba38cc'),
    'receiver': ('native-release-1.1.2/companion', 'ae969dc6f9857f58e29b4593dbaf9ad82a15d325b4c1619fb2c5636989b195ed', 'dcc7b978aafce429a3047a541f324b641e7907fe39601d0abf9f6d438a671536', '7b984e911266dee07486d1c712784671a78a03ed', '434ab4d7d29d8e9ba689837358c8a44996ba38cc'),
}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def package(root=ROOT, destination=None, check_only=False):
    source = root / SOURCE
    manifest_bytes = (source / 'manifest.json').read_bytes()
    if digest(manifest_bytes) != MANIFEST_SHA256:
        raise ValueError('Firmware manifest is outside the release allowlist')
    manifest = json.loads(manifest_bytes)
    expected = [(role, layout, f'{role}-{layout}')
                for layout in ('ansi', 'iso', 'jis', 'kr')
                for role in ('left', 'right')]
    expected.append(('receiver', 'ansi', 'receiver'))
    if manifest['version'] != VERSION or len(manifest['images']) != len(expected):
        raise ValueError('Firmware version or image count differs')
    files = {'manifest.json': manifest_bytes}
    for image, (role, layout, stem) in zip(manifest['images'], expected):
        if (image['role'], image['layout'], image['uf2'], image['binary']) != (
                role, layout, f'{stem}.uf2', f'{stem}.bin'):
            raise ValueError('Firmware image order or name differs')
        if (image['source_commit'], image['rmk_revision']) != ROLES[role][3:]:
            raise ValueError('Firmware source differs')
        for name, key in ((image['uf2'], 'uf2_sha256'),
                          (image['binary'], 'binary_sha256')):
            path = source / name
            if path.is_symlink():
                raise ValueError('Firmware file must not be a symlink')
            data = path.read_bytes()
            if digest(data) != image[key]:
                raise ValueError('Firmware file is outside the release allowlist')
            files[name] = data
        if role == 'receiver':
            proof = inspect_receiver(files[image['uf2']])
        else:
            proof = inspect_startup_image(files[image['uf2']], files[image['binary']], role)
        if int(proof['end_exclusive'], 16) != image['end_exclusive']:
            raise ValueError('Firmware address range differs')
        if f'NocFree RMK;fw={VERSION}'.encode() not in files[image['binary']]:
            raise ValueError('Firmware version descriptor differs')
    if not check_only:
        destination = destination or root / 'dist/companion-firmware'
        if destination.exists():
            raise ValueError('Firmware destination already exists')
        destination.mkdir(parents=True)
        for name, data in files.items():
            (destination / name).write_bytes(data)
    return manifest['release_id'], MANIFEST_SHA256


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true', help='Validate all evidence without generating files.')
    args = parser.parse_args()
    try:
        release_id, manifest_hash = package(check_only=args.check)
        print(f'{release_id}: verified release manifest SHA256 {manifest_hash}')
    except (ValueError, OSError, KeyError, json.JSONDecodeError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
