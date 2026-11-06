// Public room state is deliberately limited to music metadata and transport.
export function validState(s) {
  return s === null || (s && typeof s.id === 'string' && /^[A-Za-z0-9_-]{11}$/.test(s.id)
    && typeof s.title === 'string' && s.title.length <= 512
    && typeof s.artist === 'string' && s.artist.length <= 512
    && typeof s.playing === 'boolean' && Number.isFinite(s.position)
    && s.position >= 0 && s.position <= 604800
    && (s.audio_alternative === undefined || typeof s.audio_alternative === 'boolean'));
}

export function publicState(s) {
  return s && { id: s.id, title: s.title, artist: s.artist, playing: s.playing, position: s.position,
    ...(s.audio_alternative ? { audio_alternative: true } : {}) };
}
