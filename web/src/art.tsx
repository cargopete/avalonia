import { JSX } from 'solid-js'

// Art behind a manifest (RFC-AVL-001 DR-3). Each terrain/NPC can be backed by
// a real illustrated image; until one is dropped in, a stylised SVG stands in.
// To use real art: put a file under web/public/art/ and set `img` here.

type TerrainArt = { img?: string; sky: [string, string]; ground: string; accent: string }
type NpcArt = { img?: string; tint: string; hat: 'cap' | 'peaked' | 'bowler' | 'none'; glasses?: boolean }

export const TERRAIN_ART: Record<string, TerrainArt> = {
  moor: { sky: ['#3a4a52', '#8a8a76'], ground: '#2c2a20', accent: '#b9a96a' },
  fen: { sky: ['#2e3b3b', '#6f7e74'], ground: '#23291f', accent: '#7fa07a' },
  forest: { sky: ['#1f2a22', '#3a4a36'], ground: '#161c14', accent: '#5e7a4a' },
  coast: { sky: ['#26323f', '#7c8a92'], ground: '#1c2228', accent: '#9ab0bd' },
  pass: { sky: ['#2b3340', '#9aa6b4'], ground: '#20242c', accent: '#c2ccd8' },
}

const FALLBACK_TERRAIN: TerrainArt = { sky: ['#2a2820', '#6a6450'], ground: '#1c1a14', accent: '#c9a227' }

export const NPC_ART: Record<string, NpcArt> = {
  arthur: { tint: '#9a8c5a', hat: 'cap' },
  finch: { tint: '#7e8aa0', hat: 'bowler', glasses: true },
  hobbs: { tint: '#8a5a4a', hat: 'peaked' },
  carver: { tint: '#6a7e6a', hat: 'bowler' },
  tansy: { tint: '#a06a7e', hat: 'none' },
  wray: { tint: '#8a7a5a', hat: 'cap' },
}

const FALLBACK_NPC: NpcArt = { tint: '#8a8068', hat: 'none' }

/** A layered, parallax-friendly terrain backdrop. */
export function Backdrop(props: { terrain: string }): JSX.Element {
  const a = () => TERRAIN_ART[props.terrain] ?? FALLBACK_TERRAIN
  return (
    <svg class="backdrop" viewBox="0 0 800 360" preserveAspectRatio="xMidYMid slice" aria-hidden="true">
      <defs>
        <linearGradient id="sky" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stop-color={a().sky[0]} />
          <stop offset="100%" stop-color={a().sky[1]} />
        </linearGradient>
      </defs>
      <rect width="800" height="360" fill="url(#sky)" />
      {terrainLayers(props.terrain, a())}
      <rect width="800" height="360" fill="#000" opacity="0.18" />
    </svg>
  )
}

function terrainLayers(id: string, a: TerrainArt): JSX.Element {
  switch (id) {
    case 'moor':
      return (
        <>
          <circle cx="640" cy="90" r="46" fill={a.accent} opacity="0.5" />
          <path d="M0 250 Q200 200 400 245 T800 235 V360 H0Z" fill={a.ground} opacity="0.7" />
          <path d="M0 290 Q260 250 520 290 T800 285 V360 H0Z" fill={a.ground} />
          <rect x="392" y="225" width="6" height="40" fill="#000" opacity="0.5" />
        </>
      )
    case 'fen':
      return (
        <>
          <rect y="250" width="800" height="110" fill={a.ground} />
          <rect y="250" width="800" height="110" fill={a.accent} opacity="0.10" />
          {[120, 300, 520, 700].map((x) => (
            <line x1={x} y1="248" x2={x} y2="210" stroke={a.accent} stroke-width="2" opacity="0.6" />
          ))}
          <rect y="246" width="800" height="3" fill="#fff" opacity="0.08" />
        </>
      )
    case 'forest':
      return (
        <>
          {[40, 150, 250, 560, 680, 770].map((x, i) => (
            <rect x={x} y={120 - (i % 2) * 30} width={26 + (i % 3) * 6} height="240" fill={a.ground} opacity={0.85 - (i % 3) * 0.15} />
          ))}
          <path d="M0 0 H800 V120 Q400 180 0 120Z" fill="#000" opacity="0.35" />
        </>
      )
    case 'coast':
      return (
        <>
          <rect y="210" width="800" height="150" fill={a.ground} />
          <rect y="210" width="800" height="60" fill={a.accent} opacity="0.18" />
          {[235, 258, 281].map((y) => (
            <line x1="0" y1={y} x2="800" y2={y} stroke="#fff" stroke-width="1.5" opacity="0.10" />
          ))}
          <circle cx="690" cy="150" r="4" fill="#ffd76a" opacity="0.9" />
          <line x1="690" y1="150" x2="690" y2="210" stroke="#ffd76a" stroke-width="1" opacity="0.4" />
        </>
      )
    case 'pass':
      return (
        <>
          <path d="M0 360 L180 140 L320 360Z" fill={a.ground} />
          <path d="M240 360 L460 90 L640 360Z" fill={a.ground} opacity="0.9" />
          <path d="M520 360 L720 160 L800 300 V360Z" fill={a.ground} opacity="0.8" />
          <path d="M420 130 L460 90 L500 130 L470 140Z" fill="#fff" opacity="0.5" />
        </>
      )
    default:
      return <path d="M0 270 H800 V360 H0Z" fill={a.ground} />
  }
}

/** A framed, stylised portrait card. */
export function Portrait(props: { id: string; name: string; role?: string; speaking?: boolean }): JSX.Element {
  const a = () => NPC_ART[props.id] ?? FALLBACK_NPC
  return (
    <div class={`portrait ${props.speaking ? 'speaking' : ''}`}>
      <svg viewBox="0 0 140 160" aria-hidden="true">
        <defs>
          <linearGradient id={`pg-${props.id}`} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stop-color={a().tint} stop-opacity="0.55" />
            <stop offset="100%" stop-color="#0e0d09" />
          </linearGradient>
        </defs>
        <rect width="140" height="160" fill={`url(#pg-${props.id})`} />
        {/* shoulders */}
        <path d="M18 160 Q70 110 122 160Z" fill="#0e0d09" />
        <path d="M26 160 Q70 118 114 160Z" fill={a().tint} opacity="0.35" />
        {/* head */}
        <ellipse cx="70" cy="74" rx="30" ry="35" fill="#0e0d09" />
        <ellipse cx="70" cy="74" rx="27" ry="32" fill={a().tint} opacity="0.45" />
        {/* hat */}
        {hat(a().hat, a().tint)}
        {/* glasses */}
        {a().glasses && (
          <g stroke="#0e0d09" stroke-width="2" fill="none" opacity="0.8">
            <circle cx="59" cy="74" r="7" />
            <circle cx="81" cy="74" r="7" />
            <line x1="66" y1="74" x2="74" y2="74" />
          </g>
        )}
        {/* eyes */}
        <circle cx="59" cy="74" r="2.2" fill="#0e0d09" />
        <circle cx="81" cy="74" r="2.2" fill="#0e0d09" />
      </svg>
      <div class="portrait-name">{props.name}</div>
      {props.role && <div class="portrait-role">{props.role}</div>}
    </div>
  )
}

function hat(kind: NpcArt['hat'], tint: string): JSX.Element | null {
  switch (kind) {
    case 'cap':
      return <path d="M40 52 Q70 30 100 52 L104 56 Q70 46 36 56Z" fill="#0e0d09" />
    case 'peaked':
      return (
        <>
          <path d="M40 50 Q70 28 100 50 L100 54 L40 54Z" fill="#0e0d09" />
          <rect x="40" y="54" width="60" height="5" fill={tint} opacity="0.5" />
        </>
      )
    case 'bowler':
      return (
        <>
          <ellipse cx="70" cy="50" rx="30" ry="9" fill="#0e0d09" />
          <path d="M48 50 Q70 24 92 50Z" fill="#0e0d09" />
        </>
      )
    default:
      return null
  }
}
