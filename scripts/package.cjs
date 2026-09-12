const { execSync } = require('child_process')
const fs = require('fs')
const os = require('os')
const path = require('path')

const isWindows = process.platform === 'win32'

function run(cmd, args, extraEnv) {
  const env = { ...process.env, ...extraEnv }
  console.log(`Running: ${cmd} ${args.join(' ')}`)
  execSync([cmd, ...args].join(' '), {
    stdio: 'inherit',
    env,
  })
}

function linuxdeployArch() {
  return process.arch === 'arm64' ? 'aarch64' : 'x86_64'
}

function wrapCachedLinuxdeploy() {
  const arch = linuxdeployArch()
  const cache = path.join(os.homedir(), '.cache', 'tauri')
  const appimage = path.join(cache, `linuxdeploy-${arch}.AppImage`)
  const backup = path.join(cache, `linuxdeploy-${arch}.AppImage.bin`)
  const extractRoot = path.join(os.tmpdir(), 'linuxdeploy-extracted')
  const extractedBin = path.join(extractRoot, 'squashfs-root', 'usr', 'bin', 'linuxdeploy')

  if (fs.existsSync(appimage) && fs.statSync(appimage).size > 1_000_000) {
    fs.copyFileSync(appimage, backup)
  }
  const source = fs.existsSync(backup) ? backup : appimage
  if (!fs.existsSync(extractedBin) && fs.existsSync(source) && fs.statSync(source).size > 1_000_000) {
    fs.rmSync(extractRoot, { recursive: true, force: true })
    fs.mkdirSync(extractRoot, { recursive: true })
    console.log(`linuxdeploy: extracting ${source}`)
    execSync(
      `APPIMAGE_EXTRACT_AND_RUN=1 ${JSON.stringify(source)} --appimage-extract`,
      { stdio: 'inherit', cwd: extractRoot },
    )
  }
  if (!fs.existsSync(extractedBin)) {
    return null
  }

  // Tauri always `dd`s 3 zero bytes at offset 8 of the cache AppImage (to
  // hide it from desktop integration). A shell wrapper would be corrupted;
  // put a no-op `dd` first on PATH for that of= target.
  const binDir = path.join(os.tmpdir(), 'take-a-moment-bin')
  fs.mkdirSync(binDir, { recursive: true })
  const dd = path.join(binDir, 'dd')
  fs.writeFileSync(
    dd,
    `#!/bin/sh
for a in "$@"; do
  case "$a" in
    of=*linuxdeploy*) exit 0 ;;
  esac
done
exec /usr/bin/dd "$@"
`,
  )
  fs.chmodSync(dd, 0o755)

  fs.writeFileSync(
    appimage,
    `#!/bin/bash
# Static linuxdeploy still walks /usr/bin via AppImage AppRun. Run the
# extracted ELF with a PATH that cannot see unreadable sentinelctl.
# Keep ~/.cache/tauri on PATH so gtk/gstreamer plugins next to the
# original AppImage are still found.
# Tauri always passes --appimage-extract-and-run; that is an AppImage
# runtime flag and the extracted ELF rejects it.
PATH=$(printf '%s' "$PATH" | tr ':' '\\n' | grep -v -x -e /usr/bin -e /bin | paste -sd:)
export PATH=${JSON.stringify(cache)}:${JSON.stringify(path.join(extractRoot, 'squashfs-root', 'usr', 'bin'))}:"$PATH"
args=()
for a in "$@"; do
  case "$a" in
    --appimage-extract-and-run|--appimage-extract) continue ;;
  esac
  args+=("$a")
done
exec ${JSON.stringify(extractedBin)} "\${args[@]}"
`,
  )
  fs.chmodSync(appimage, 0o755)
  console.log(`linuxdeploy: wrapped ${appimage} -> ${extractedBin}`)
  return binDir
}

function linuxDeployEnv() {
  // linuxdeploy on Ubuntu 24 dies on .relr.dyn when it strips bundled libs.
  const env = {
    NO_STRIP: process.env.NO_STRIP || 'true',
    APPIMAGE_EXTRACT_AND_RUN: process.env.APPIMAGE_EXTRACT_AND_RUN || '1',
  }

  const wrapBin = wrapCachedLinuxdeploy()

  // linuxdeploy walks every PATH directory. On usr-merged hosts with
  // SentinelOne, `/usr/bin/sentinelctl` exists but is unreadable, and
  // boost::filesystem::status then aborts the AppImage bundle.
  const pathDirs = (process.env.PATH || '').split(path.delimiter).filter(Boolean)
  const blocked = new Set()
  for (const dir of pathDirs) {
    const resolved = path.resolve(dir)
    let names
    try {
      names = fs.readdirSync(resolved)
    } catch {
      continue
    }
    for (const name of names) {
      try {
        fs.statSync(path.join(resolved, name))
      } catch (err) {
        if (err && (err.code === 'EACCES' || err.code === 'EPERM')) {
          blocked.add(resolved)
          break
        }
      }
    }
  }

  if (blocked.size === 0) {
    if (wrapBin) {
      env.PATH = [wrapBin, process.env.PATH].filter(Boolean).join(path.delimiter)
    }
    return env
  }

  const farmRoot = path.join(os.tmpdir(), 'take-a-moment-linuxdeploy-path')
  const replacements = new Map()
  for (const dir of blocked) {
    const farm = path.join(farmRoot, dir.replace(/[\\/]/g, '_'))
    fs.rmSync(farm, { recursive: true, force: true })
    fs.mkdirSync(farm, { recursive: true })
    for (const name of fs.readdirSync(dir)) {
      if (name === 'sentinelctl') {
        continue
      }
      const src = path.join(dir, name)
      try {
        const st = fs.lstatSync(src)
        // usr-merge `/usr/bin/X11 -> .` would make linuxdeploy walk the
        // real /usr/bin (and hit sentinelctl) from inside the farm.
        if (st.isSymbolicLink()) {
          const target = fs.readlinkSync(src)
          if (target === '.' || path.resolve(dir, target) === path.resolve(dir)) {
            continue
          }
        }
        fs.statSync(src)
      } catch {
        continue
      }
      try {
        fs.symlinkSync(src, path.join(farm, name))
      } catch {
        // skip colliding names (usr-merge duplicates)
      }
    }
    replacements.set(dir, farm)
    console.log(
      `linuxdeploy PATH: hiding unreadable entries in ${dir} via ${farm}`
    )
  }

  env.PATH = pathDirs
    .map((dir) => replacements.get(path.resolve(dir)) || dir)
    .join(path.delimiter)
  if (wrapBin) {
    env.PATH = wrapBin + path.delimiter + env.PATH
  }
  return env
}

// `npm run package` is used for both Windows and Linux builds.
// The original repo hard-coded Windows NSIS packaging; this script makes it
// OS-aware so you can generate Ubuntu 24 installers too.
if (isWindows) {
  run('tauri', [
    'build',
    '--bundles',
    'nsis',
    '--target',
    'x86_64-pc-windows-gnu',
  ])
  run('node', ['scripts/post-package.cjs'])
} else {
  const linuxEnv = linuxDeployEnv()
  // .deb first so a linuxdeploy failure still leaves an installable package.
  run('tauri', ['build', '--bundles', 'deb'], linuxEnv)
  run('tauri', ['build', '--bundles', 'appimage'], linuxEnv)
  const arch = linuxdeployArch()
  const backup = path.join(os.homedir(), '.cache', 'tauri', `linuxdeploy-${arch}.AppImage.bin`)
  const appimage = path.join(os.homedir(), '.cache', 'tauri', `linuxdeploy-${arch}.AppImage`)
  if (fs.existsSync(backup)) {
    fs.copyFileSync(backup, appimage)
    fs.chmodSync(appimage, 0o755)
    console.log(`linuxdeploy: restored ${appimage}`)
  }
  run('node', ['scripts/post-package.cjs'], linuxEnv)
}
