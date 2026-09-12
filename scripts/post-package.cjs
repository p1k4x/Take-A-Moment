const fs = require('fs')
const path = require('path')
const { version } = require('../package.json')

const releaseDir = path.join(__dirname, '..', 'release')
fs.mkdirSync(releaseDir, { recursive: true })

function copyArtifact(src, destName) {
  const dest = path.join(releaseDir, destName)
  fs.copyFileSync(src, dest)
  const mode = fs.statSync(src).mode
  fs.chmodSync(dest, mode)
  console.log(`Copied ${src} -> ${dest}`)
}

if (process.platform === 'win32') {
  const src = path.join(
    __dirname,
    '..',
    'src-tauri',
    'target',
    'x86_64-pc-windows-gnu',
    'release',
    'bundle',
    'nsis',
    `Take A Moment_${version}_x64-setup.exe`
  )
  copyArtifact(src, `Take A Moment Setup ${version}.exe`)
  process.exit(0)
}

const bundleRoot = path.join(
  __dirname,
  '..',
  'src-tauri',
  'target',
  'release',
  'bundle'
)

const debDir = path.join(bundleRoot, 'deb')
const debFiles = fs.existsSync(debDir)
  ? fs.readdirSync(debDir).filter((name) => name.endsWith('.deb'))
  : []
if (debFiles.length === 0) {
  throw new Error(`No .deb found in ${debDir}`)
}
for (const name of debFiles) {
  copyArtifact(path.join(debDir, name), name.replace(/ /g, '-'))
}

const appimageDir = path.join(bundleRoot, 'appimage')
const appimageFiles = fs.existsSync(appimageDir)
  ? fs.readdirSync(appimageDir).filter((name) => name.endsWith('.AppImage'))
  : []
if (appimageFiles.length === 0) {
  throw new Error(`No .AppImage found in ${appimageDir}`)
}
for (const name of appimageFiles) {
  copyArtifact(path.join(appimageDir, name), name.replace(/ /g, '-'))
}
