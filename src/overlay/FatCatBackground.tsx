import { useEffect, useRef, useState, type CSSProperties, type SyntheticEvent } from 'react'
import { invoke } from '@tauri-apps/api/core'

function getVideoUrl(filename: string): string {
  return new URL(`../videos/${filename}`, location.href).href
}

function logFatCat(message: string) {
  console.error(message)
  void invoke('log_overlay', { message })
}

function logVideoError(ev: SyntheticEvent<HTMLVideoElement>) {
  const video = ev.currentTarget
  const err = video.error
  logFatCat(
    `[fatcat] video error src=${video.currentSrc} code=${err?.code} message=${err?.message} networkState=${video.networkState}`,
  )
}

const fillStyle: CSSProperties = {
  position: 'absolute',
  inset: 0,
  width: '100%',
  height: '100%',
  objectFit: 'cover',
}

function videoStyle(visible: boolean): CSSProperties {
  return {
    ...fillStyle,
    // display:none (not opacity/z-index) — these videos have transparent
    // backgrounds, so anything merely "behind" or faded would still show
    // through the intro's transparent pixels. Fully absent from the render
    // tree is the only way to hide it.
    display: visible ? 'block' : 'none',
  }
}

const IS_LINUX = new URLSearchParams(window.location.search).get('linux') === '1'

export function FatCatBackground() {
  if (IS_LINUX) {
    return <LinuxLoop />
  }
  return <WindowsHandover />
}

// WebKitGTK cannot start a second VP9-alpha pipeline in the same webview:
// src-swap shows one stretched opaque frame (alpha lost, buttons sit on black)
// and then stalls. Play the loop clip only so the first pipeline keeps alpha.
function LinuxLoop() {
  const videoRef = useRef<HTMLVideoElement>(null)
  const [src, setSrc] = useState<string>()
  const restRef = useRef<string[]>([])

  // AppImage WebKitGTK rejects custom schemes (tauri://, asset://, blob:) with
  // code=4. Serve the clip over loopback HTTP (souphttpsrc) and fall back to
  // file:// (filesrc), then the bundled tauri:// URL last.
  useEffect(() => {
    let cancelled = false
    void (async () => {
      logFatCat(`[fatcat] location=${location.href}`)
      const bundled = getVideoUrl('neko2.webm')
      try {
        const urls = await invoke<string[]>('fatcat_video_urls', { filename: 'neko2.webm' })
        const list = [...urls, bundled]
        logFatCat(`[fatcat] candidates=${list.join(' | ')}`)
        if (!cancelled) {
          restRef.current = list.slice(1)
          setSrc(list[0])
        }
      } catch (e) {
        logFatCat(`[fatcat] fatcat_video_urls failed ${e}; using ${bundled}`)
        if (!cancelled) setSrc(bundled)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
    const video = videoRef.current
    if (!video || !src) return
    let cooldownUntil = 0

    // HTML `loop` and play()-after-ended are ignored for VP9-alpha. Seek back
    // before EOS so the pipeline never stops.
    const rewind = () => {
      const now = performance.now()
      if (now < cooldownUntil) return
      cooldownUntil = now + 280
      try {
        video.currentTime = 0.05
      } catch {
        return
      }
      if (video.paused) {
        const play = video.play()
        if (play) void play.catch(() => {})
      }
    }

    const onTimeUpdate = () => {
      if (!Number.isFinite(video.duration) || video.duration <= 0) return
      if (video.currentTime >= video.duration - 0.22) rewind()
    }

    const onEnded = () => rewind()
    const onPause = () => {
      if (video.currentTime > 1) rewind()
    }

    video.addEventListener('timeupdate', onTimeUpdate)
    video.addEventListener('ended', onEnded)
    video.addEventListener('pause', onPause)
    return () => {
      video.removeEventListener('timeupdate', onTimeUpdate)
      video.removeEventListener('ended', onEnded)
      video.removeEventListener('pause', onPause)
    }
  }, [src])

  if (!src) return null

  const handleError = (ev: SyntheticEvent<HTMLVideoElement>) => {
    logVideoError(ev)
    const next = restRef.current.shift()
    if (next) {
      logFatCat(`[fatcat] next src=${next}`)
      setSrc(next)
    }
  }

  return (
    <video
      ref={videoRef}
      src={src}
      autoPlay
      muted
      playsInline
      onError={handleError}
      onPlaying={() => logFatCat(`[fatcat] playing src=${src}`)}
      style={fillStyle}
    />
  )
}

function WindowsHandover() {
  const loopRef = useRef<HTMLVideoElement>(null)
  const [showLoop, setShowLoop] = useState(false)
  const [hideIntro, setHideIntro] = useState(false)

  const handleIntroEnded = () => {
    const loop = loopRef.current
    if (loop) {
      loop.currentTime = 0
      void loop.play()
    }
    setShowLoop(true)
    requestAnimationFrame(() => setHideIntro(true))
  }

  return (
    <div style={{ position: 'absolute', inset: 0, overflow: 'hidden' }}>
      <video
        src={getVideoUrl('neko1.webm')}
        autoPlay
        muted
        playsInline
        onEnded={handleIntroEnded}
        style={videoStyle(!hideIntro)}
      />
      <video
        ref={loopRef}
        src={getVideoUrl('neko2.webm')}
        muted
        playsInline
        loop
        preload="auto"
        style={videoStyle(showLoop)}
      />
    </div>
  )
}
