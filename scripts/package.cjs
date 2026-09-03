const { execSync } = require('child_process')

const isWindows = process.platform === 'win32'

function run(cmd, args) {
  console.log(`Running: ${cmd} ${args.join(' ')}`)
  execSync([cmd, ...args].join(' '), {
    stdio: 'inherit',
    env: process.env,
  })
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
  run('tauri', [
    'build',
    '--bundles',
    'deb',
    'appimage',
  ])
}

