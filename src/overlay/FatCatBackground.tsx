import { useEffect, useRef, useState, type CSSProperties } from 'react'

function getVideoUrl(filename: string): string {
  return new URL(`../videos/${filename}`, location.href).href
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

  useEffect(() => {
    const video = videoRef.current
    if (!video) return
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
  }, [])

  return (
    <video
      ref={videoRef}
      src={getVideoUrl('neko2.webm')}
      autoPlay
      muted
      playsInline
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
