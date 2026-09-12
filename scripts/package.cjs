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

function linuxDeployEnv() {
  // linuxdeploy on Ubuntu 24 dies on .relr.dyn when it strips bundled libs.
  const env = {
    NO_STRIP: process.env.NO_STRIP || 'true',
    APPIMAGE_EXTRACT_AND_RUN: process.env.APPIMAGE_EXTRACT_AND_RUN || '1',
  }

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
  run('node', ['scripts/post-package.cjs'], linuxEnv)
}
