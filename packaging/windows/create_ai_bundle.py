#!/usr/bin/env python3
"""Stage the independent AI Windows bundle and generate WiX 3/4 input. No runtime downloads."""
import argparse
import hashlib
import json
import pathlib
import shutil
import struct
import subprocess
import sys
import tomllib
import uuid
import xml.etree.ElementTree as ET
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
UPGRADE = '777570db-86cd-49dd-9963-880b2082c7c0'  # independent from the official MSI


def pe(path, subsystem):
    with path.open('rb') as f:
        header = f.read(4096)
    if len(header) < 64 or header[:2] != b'MZ':
        raise ValueError(f'{path.name}: not a Windows executable')
    off = struct.unpack_from('<I', header, 60)[0]
    if off > len(header) - 96 or header[off:off + 4] != b'PE\0\0':
        raise ValueError(f'{path.name}: invalid PE header')
    machine = struct.unpack_from('<H', header, off + 4)[0]
    kind = struct.unpack_from('<H', header, off + 92)[0]
    if machine != 0x8664 or kind != subsystem:
        raise ValueError(f'{path.name}: expected x64 subsystem {subsystem}, got {machine:#x}/{kind}')


def sha(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def wix_document(stage, version, commit, fmt):
    ns = 'http://wixtoolset.org/schemas/v4/wxs' if fmt == 4 else 'http://schemas.microsoft.com/wix/2006/wi'
    ET.register_namespace('', ns)

    def add(parent, tag, **attrs):
        return ET.SubElement(parent, '{' + ns + '}' + tag, {k: str(v) for k, v in attrs.items()})

    root = ET.Element('{' + ns + '}Wix')
    name = 'FilmCraft AI'
    if fmt == 4:
        product = add(root, 'Package', Name=name, Manufacturer='FilmCraft AI contributors', Version=version,
                      UpgradeCode=UPGRADE, Scope='perUser', Compressed='yes', Language='1033')
        add(product, 'MediaTemplate', EmbedCab='yes')
        base = add(product, 'StandardDirectory', Id='LocalAppDataFolder')
        add(product, 'StandardDirectory', Id='ProgramMenuFolder')
    else:
        product = add(root, 'Product', Id=str(uuid.uuid5(uuid.UUID(UPGRADE), commit)), Name=name, Language='1033',
                      Version=version, Manufacturer='FilmCraft AI contributors', UpgradeCode=UPGRADE)
        add(product, 'Package', InstallerVersion='500', Compressed='yes', InstallScope='perUser')
        add(product, 'Media', Id='1', Cabinet='bundle.cab', EmbedCab='yes')
        target = add(product, 'Directory', Id='TARGETDIR', Name='SourceDir')
        base = add(target, 'Directory', Id='LocalAppDataFolder')
        add(target, 'Directory', Id='ProgramMenuFolder')
    add(product, 'MajorUpgrade', AllowSameVersionUpgrades='yes', DowngradeErrorMessage='A newer FilmCraft AI build is installed.')
    folder = add(add(base, 'Directory', Id='ProgramsFolder', Name='Programs'), 'Directory', Id='INSTALLFOLDER', Name='FilmCraftAI')
    feature = add(product, 'Feature', Id='Main', Title=name, Level='1')
    add(product, 'Icon', Id='FilmcraftIcon.ico', SourceFile=str(ROOT / 'assets/app-icon/filmcraft.ico'))
    add(product, 'Property', Id='ARPPRODUCTICON', Value='FilmcraftIcon.ico')
    add(product, 'Property', Id='ARPURLINFOABOUT', Value='https://github.com/cirocueto2013-wq/filmcraft/tree/build/windows-ai-bundle')
    for index, path in enumerate(sorted(stage.iterdir())):
        if not path.is_file():
            raise ValueError('Bundle must be flat: ' + str(path))
        ident = 'Payload' + str(index)
        attrs = {'Id': ident, 'Guid': str(uuid.uuid5(uuid.UUID(UPGRADE), path.name))}
        if fmt == 3:
            attrs['Win64'] = 'yes'
        component = add(folder, 'Component', **attrs)
        file = add(component, 'File', Id=ident + 'File', Name=path.name, Source=str(path), KeyPath='yes')
        if path.name == 'FilmCraft-Codex.cmd':
            add(file, 'Shortcut', Id='CodexShortcut', Directory='ProgramMenuFolder', Name='FilmCraft AI (Codex)',
                Description='FilmCraft AI with local Codex MCP bridge', WorkingDirectory='INSTALLFOLDER',
                Advertise='yes', Icon='FilmcraftIcon.ico', IconIndex='0')
        add(feature, 'ComponentRef', Id=ident)
    return ET.ElementTree(root)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--bin-dir', type=pathlib.Path, required=True)
    ap.add_argument('--dist', type=pathlib.Path, default=ROOT / 'dist/ai')
    ap.add_argument('--wix-format', type=int, choices=(3, 4), default=4)
    ap.add_argument('--wixl', help='Compile MSI with this Linux wixl binary (WiX format 3 only)')
    ap.add_argument('--allow-dirty', action='store_true', help='Private verification only: clearly mark an uncommitted source tree')
    args = ap.parse_args()
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
    numeric = version.split('-')[0]
    if len(numeric.split('.')) != 3 or not all(s.isdigit() for s in numeric.split('.')):
        raise ValueError('Version is not compatible with Windows Installer')
    commit = subprocess.check_output(['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], text=True).strip()
    dirty = bool(subprocess.check_output(['git', '-C', str(ROOT), 'status', '--porcelain'], text=True).strip())
    if dirty and not args.allow_dirty:
        raise ValueError('Commit the source tree before packaging; --allow-dirty is only for private verification')
    name = f'filmcraft-{version}-ai-{commit[:8]}-windows-x64'
    dist = args.dist.resolve(); dist.mkdir(parents=True, exist_ok=True)
    stage = dist / name
    if stage.exists():
        raise ValueError(f'{stage} already exists; use a new output directory, existing builds are preserved')
    for filename, subsystem in [('filmcraft.exe', 2), ('filmcraft-cli.exe', 3)]:
        pe(args.bin_dir / filename, subsystem)
    stage.mkdir()
    for filename in ('filmcraft.exe', 'filmcraft-cli.exe'):
        shutil.copy2(args.bin_dir / filename, stage / filename)
    for path in (ROOT / 'packaging/windows/ai-bundle').iterdir():
        if path.is_file():
            shutil.copy2(path, stage / path.name)
    for source, dest in [('docs/windows-ai-setup.md', 'README.md'), ('docs/ai-and-mcp.md', 'AI-MCP.md'),
                         ('LICENSE-MIT', 'LICENSE-MIT'), ('LICENSE-APACHE', 'LICENSE-APACHE'), ('ATTRIBUTION.md', 'ATTRIBUTION.md')]:
        shutil.copy2(ROOT / source, stage / dest)
    info = {'name': 'FilmCraft AI', 'version': version, 'commit': commit, 'sourceDirty': dirty,
            'platform': 'windows-x64', 'aiAndMcp': True, 'forkBuild': True,
            'servicesBundled': False, 'files': {p.name: sha(p) for p in sorted(stage.iterdir()) if p.is_file()}}
    (stage / 'BUILD_INFO.json').write_text(json.dumps(info, indent=2) + '\n', encoding='utf-8')
    source = dist / (name + '.wxs')
    wix_document(stage, numeric, commit, args.wix_format).write(source, encoding='utf-8', xml_declaration=True)
    archive = dist / (name + '-portable.zip')
    with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED, compresslevel=6) as z:
        for path in sorted(stage.iterdir()):
            z.write(path, name + '/' + path.name)
    if args.wixl:
        if args.wix_format != 3:
            raise ValueError('wixl needs --wix-format 3')
        subprocess.run([args.wixl, '-a', 'x64', '-o', str(dist / (name + '.msi')), str(source)], check=True)
    outputs = {p.name: sha(p) for p in sorted(dist.iterdir()) if p.is_file() and p.suffix in ('.msi', '.zip')}
    (dist / 'SHA256SUMS.txt').write_text(''.join(f'{digest}  {filename}\n' for filename, digest in outputs.items()))
    print(json.dumps({'stage': str(stage), 'wxs': str(source), 'zip': str(archive), 'commit': commit, 'sourceDirty': dirty}))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f'AI packaging failed: {error}', file=sys.stderr)
        sys.exit(1)
