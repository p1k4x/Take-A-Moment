import { useEffect, useRef, useState, type CSSProperties, type SyntheticEvent } from 'react'

function getVideoUrl(filename: string): string {
  return new URL(`../videos/${filename}`, location.href).href
}

function videoStyle(visible: boolean): CSSProperties {
  return {
    position: 'absolute',
    inset: 0,
    width: '100%',
    height: '100%',
    objectFit: 'cover',
    // display:none (not opacity/z-index) — these videos have transparent
    // backgrounds, so anything merely "behind" or faded would still show
    // through the intro's transparent pixels. Fully absent from the render
    // tree is the only way to hide it.
    display: visible ? 'block' : 'none',
  }
}

export function FatCatBackground() {
  const loopRef = useRef<HTMLVideoElement>(null)
  const [showLoop, setShowLoop] = useState(false)
  const [hideIntro, setHideIntro] = useState(false)
  const [loopMounted, setLoopMounted] = useState(false)

  const handleIntroTimeUpdate = (event: SyntheticEvent<HTMLVideoElement>) => {
    if (loopMounted) return
    const video = event.currentTarget
    if (!Number.isFinite(video.duration) || video.duration <= 0) return
    // Decode only one 1080p VP9-alpha stream during the slide-in; mount the
    // loop clip shortly before the intro ends so handover can still be seamless.
    if (video.duration - video.currentTime <= 1.5) {
      setLoopMounted(true)
    }
  }

  const handleIntroEnded = () => {
    setLoopMounted(true)
    setShowLoop(true)
    // One frame of overlap (imperceptible) before removing the intro —
    // this is the original cat-gatekeeper's own handover technique.
    requestAnimationFrame(() => setHideIntro(true))
  }

  useEffect(() => {
    if (!showLoop) return
    const loop = loopRef.current
    if (!loop) return
    loop.currentTime = 0
    void loop.play()
  }, [showLoop, loopMounted])

  return (
    <div style={{ position: 'absolute', inset: 0, overflow: 'hidden' }}>
      <video
        src={getVideoUrl('neko1.webm')}
        autoPlay
        muted
        playsInline
        onTimeUpdate={handleIntroTimeUpdate}
        onEnded={handleIntroEnded}
        style={videoStyle(!hideIntro)}
      />
      {loopMounted && (
        <video
          ref={loopRef}
          src={getVideoUrl('neko2.webm')}
          muted
          playsInline
          loop
          preload="auto"
          style={videoStyle(showLoop)}
        />
      )}
    </div>
  )
}
