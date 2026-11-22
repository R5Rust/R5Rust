// r5rust's live HUD bridge: runs Respawn's own RUI code (weapon_hud_v2, crosshair_plus) through apex-content-kit's
// rui-host (ruihost.js, copied from the kit) and answers with decoded draw lists. One JSON request per stdin line,
// one JSON answer per stdout line (same id). Started by crates/client/src/rui.rs.
//   {id, kind: 'hud', screen: [w, h], weapon: 'r99', mag, magMax, tier, attachments: {barrel|mag|sight|grip: {id, tier}|null}}
//   {id, kind: 'hud', screen, slots: [slot|null, slot|null], active: 0|1|-1, selected}   the inventory's loadout (crates/client/src/loot.rs):
//      slot = {ref: 'mp_weapon_r97', mag, magMax, reserve: n|'inf', reserveMax, tier, attachments: {point: {ref, tier}}}
//   {id, kind: 'crosshair', ui: 'crosshair_plus', screen: [w, h], args: {adjustedSpread, adsFrac, playerFov, ...}}
// Loadout data (icons, empty-slot art, which attachments fit) comes from the kit's public/js/data.js.
import { createRequire } from 'node:module'
import { pathToFileURL } from 'node:url'
import fs from 'node:fs'
import path from 'node:path'
import readline from 'node:readline'

const KIT = 'C:/Code/apex/apex-content-kit'
const require = createRequire(import.meta.url)
const rui = require('./ruihost.js')
globalThis.fetch = async u => ({ json: async () => JSON.parse(fs.readFileSync(path.join(KIT, 'public', u), 'utf8')) })
const D = await import(pathToFileURL(path.join(KIT, 'public/js/data.js')).href)
await D.loadData()

const ruiPath = src => src ? 'rui/' + src.replace(/^game\/icons\//, '').replace(/\.png$/, '') : ''

// the kit's gamehud.js toSlot(), for r5rust's weapon state
function slotFor (q) {
  const w = D.byId(D.WEAPONS, q.weapon || 'r99')
  if (!w) throw new Error(`unknown weapon ${q.weapon}`)
  const attachments = {}
  for (const p of D.POINTS) {
    if (!w.points.includes(p.id)) continue
    const k = q.attachments && q.attachments[p.id]
    const a = k && D.byId(D.ATTACHMENTS, k.id)
    attachments[p.id] = a ? { icon: ruiPath(a.icon), tier: k.tier === 'c' ? 5 : k.tier } : { icon: ruiPath(D.emptyIcon(p.id, w)), tier: 0 }
  }
  return {
    name: w.name, hudIcon: ruiPath(w.iconPath), ammo: w.ammo, ammoIcon: ruiPath((D.AMMO[w.ammo] || D.AMMO.light).icon),
    mag: q.mag ?? 0, magMax: q.magMax ?? 1, reserve: q.reserve ?? 'inf', reserveMax: q.reserveMax ?? 1, tier: q.tier ?? 1, fireMode: w.toggle || 'auto',
    mastered: false, masteredIcon: ruiPath(w.masteredPath), moreSparks: false, attachments
  }
}

// the inventory's slot (retail loot refs) -> the kit's slot: weapon by its `ref`, attachments by the kit's `fr` loot ref
// (level suffix dropped: bullets_mag_l2 -> light-mag, tier 2) or, for hop-ups, by icon name (hopup_shield_breaker)
const stem = r => (r || '').replace(/_l\d$/, '')
function attachmentByRef (ref) {
  return D.ATTACHMENTS.find(a => a.fr && stem(a.fr) === stem(ref)) ||
    D.ATTACHMENTS.find(a => path.basename(a.icon || '', '.png') === ref)
}
function slotForRef (s) {
  const w = D.WEAPONS.find(x => x.ref === s.ref)
  if (!w) throw new Error(`no kit weapon for ${s.ref}`)
  const attachments = {}
  for (const p of D.POINTS) {
    if (!w.points.includes(p.id)) continue
    const k = s.attachments && s.attachments[p.id === 'sight' ? 'sight' : p.id]
    const a = k && attachmentByRef(k.ref)
    attachments[p.id] = a ? { icon: ruiPath(a.icon), tier: k.tier } : { icon: ruiPath(D.emptyIcon(p.id, w)), tier: 0 }
  }
  return {
    name: w.name, hudIcon: ruiPath(w.iconPath), ammo: w.ammo, ammoIcon: ruiPath((D.AMMO[w.ammo] || D.AMMO.light).icon),
    mag: s.mag ?? 0, magMax: s.magMax ?? 1, reserve: s.reserve ?? 'inf', reserveMax: s.reserveMax ?? 1, tier: s.tier ?? 1,
    fireMode: w.toggle || 'auto', mastered: false, masteredIcon: ruiPath(w.masteredPath), moreSparks: false, attachments
  }
}
const FONTS = path.join(KIT, 'public/game/fonts')
function img (a) {
  if (!a) return null
  if (a.code !== undefined) return { code: a.code }
  if (a.path === 'white') return { white: true }
  return { file: a.guid ? rui.imagePath(a.guid) : null, meta: a.meta || null }
}
function slim (list) {
  return list.widgets.map(w => ({
    type: w.type, pos: w.pos, grad: w.grad, size: w.size, clipRect: w.clipRect || null, style: w.style,
    text: w.text, align: w.align || 0, img0: img(w.img0), img1: img(w.img1),
    mins: w.mins, maxs: w.maxs, uvMin: w.uvMin, uvMax: w.uvMax, flags: w.flags || 0
  }))
}

// Retail vs the S21 weapon_hud_v2 we run: retail's auto fire-mode glyph is four bullets (0x93CB6874EBC1F756, live ui.rpak)
// where S21 reused the infinity loop (weapon_toggle/automatic); retail's reserve infinity (the same loop image) sits
// flatter, centred under the mag count inside the plate (Kainz's retail shots), where S21 draws it in the 54x34 reserve
// text box hanging off the plate. INF_SCALE / INF_SHIFT: the big loop's box height factor and centre shift (box heights).
const AUTO_RETAIL = '93CB6874EBC1F756'
const INF_SCALE = [1, 1]
const INF_SHIFT = 0
function retailize (widgets) {
  for (const w of widgets) {
    if (w.type !== 1 || !w.img0 || w.img0.path !== 'rui/hud/weapon_toggle/automatic') continue
    if (w.size[0] < 40) {
      w.img0 = { path: 'rui/hud/weapon_toggle/automatic', guid: AUTO_RETAIL, meta: rui.imageMeta(AUTO_RETAIL) }
    } else {
      const [sx, sy] = INF_SCALE
      const [gx, gy] = [w.grad[0], w.grad[3]]
      w.pos = [w.pos[0] + gx * (1 - sx) / 2, w.pos[1] + gy * (1 - sy) / 2 + gy * INF_SHIFT]
      w.grad = [gx * sx, w.grad[1], w.grad[2], gy * sy]
      w.size = [w.size[0] * sx, w.size[1] * sy]
    }
  }
  return widgets
}

// The RUIs are laid out on a 16:9 plane that fits the window (uniform scale, like retail at 16:10 / 3:2): the host
// stretches some widgets non-uniformly on other aspects (the reserve infinity grew past the plate in a 1548x1003
// window). The plane is anchored in the window (weapon panel: bottom right, crosshair: centre) and the widgets'
// normalised positions are mapped back onto the real screen.
function plane (screen) {
  const [W, H] = screen && screen.length === 2 ? screen : [1920, 1080]
  return W * 9 > H * 16 ? [Math.round(H * 16 / 9), H] : [W, Math.round(W * 9 / 16)]
}
function toScreen (widgets, screen, lp, anchor) {
  const [W, H] = screen
  if (!W || !H || (lp[0] === W && lp[1] === H)) return
  const [sx, sy] = [lp[0] / W, lp[1] / H]
  const [ox, oy] = [(W - lp[0]) * anchor[0] / W, (H - lp[1]) * anchor[1] / H]
  for (const w of widgets) {
    if (w.pos) w.pos = [w.pos[0] * sx + ox, w.pos[1] * sy + oy]
    if (w.grad) w.grad = [w.grad[0] * sx, w.grad[1] * sy, w.grad[2] * sx, w.grad[3] * sy]
    if (w.clipRect) w.clipRect = [w.clipRect[0] * sx + ox, w.clipRect[1] * sy + oy, w.clipRect[2] * sx + ox, w.clipRect[3] * sy + oy]
  }
}

async function handle (q) {
  if (q.kind === 'hud') {
    const lp = plane(q.screen)
    const loadout = q.slots
      ? { slots: q.slots.map(s => s ? slotForRef(s) : null), active: q.active ?? -1, selected: q.selected ?? true }
      : { slots: [slotFor(q)], active: 0 }
    const r = await rui.hud({}, lp, loadout)
    retailize(r.widgets)
    const widgets = slim(r)
    toScreen(widgets, q.screen, lp, [1, 1])
    return { hidden: r.hidden, screen: q.screen || r.screen, widgets }
  }
  if (q.kind === 'crosshair') {
    const lp = plane(q.screen)
    const r = await rui.runHost(q.ui || 'crosshair_plus', q.args || {}, lp, '', q.time ?? 100)
    const widgets = slim({ widgets: rui.drawList(r) })
    toScreen(widgets, q.screen, lp, [0.5, 0.5])
    return { hidden: !!r.j.hidden, screen: q.screen || r.screen, widgets }
  }
  // Use flyouts (ui/zipline_flyout, S30 LoadHudPromptAsset 0x1408a9680, args set in 0x14069b870): the key caps,
  // labels and the anchor dot come from the RUI; its leader line projects `usePos` with the engine's live view, which
  // the offline host doesn't have, so it is dropped here and the client draws it from the dot to the projected point.
  // `%&use%` / `%ping%` become the bound keys (q.keys) on a light key cap, like retail's prompt.
  if (q.kind === 'flyout') {
    const lp = plane(q.screen)
    const r = await rui.runHost(q.ui || 'zipline_flyout', {}, lp, '', q.time ?? 100)
    const all = slim({ widgets: rui.drawList(r) })
    const keys = q.keys || {}
    const out = []
    for (let i = 0; i < all.length; i++) {
      const w = all[i]
      // the projected leader line (degenerate offline: huge position)
      if (w.type === 1 && w.pos && Math.abs(w.pos[0]) > 10) continue
      const m = w.type === 0 && typeof w.text === 'string' && w.text.match(/^%&?([a-z_]+)%$/i)
      if (m) {
        const label = all[i + 1]
        const key = keys[m[1]] || m[1].toUpperCase()
        const [bw, bh] = [Math.max(30, 16 * key.length + 14), 30]
        const cx = label ? label.pos[0] + label.grad[0] * 0.5 : w.pos[0]
        const box = { pos: [cx - bw / lp[0] / 2, w.pos[1]], grad: [bw / lp[0], 0, 0, bh / lp[1]], size: [bw, bh] }
        out.push({
          type: 2, ...box, clipRect: null, text: null, align: 0, img0: { white: true }, img1: null,
          mins: [0, 0], maxs: [1, 1], uvMin: [0, 0], uvMax: [1, 1], flags: 0,
          style: { ...w.style, color: [[0.86, 0.86, 0.86, 0.95], [0, 0, 0, 0], [0, 0, 0, 0]], tint: [1, 1, 1, 1] }
        })
        out.push({ ...w, ...box, text: key, align: 0.5, style: { ...w.style, color: [[0.06, 0.06, 0.06, 1], [0, 0, 0, 0], [0, 0, 0, 0]] } })
        continue
      }
      out.push(w)
    }
    toScreen(out, q.screen, lp, [0.5, 0.5])
    return { hidden: !!r.j.hidden, screen: q.screen || r.screen, widgets: out }
  }
  if (q.kind === 'fonts') return { dir: FONTS }
  throw new Error(`unknown request ${q.kind}`)
}

const rl = readline.createInterface({ input: process.stdin })
process.stdout.write(JSON.stringify({ id: 0, ready: true, fonts: FONTS }) + '\n')
let pending = 0
let closed = false
const finish = () => { if (closed && pending === 0) process.exit(0) }
rl.on('line', line => {
  let q
  try { q = JSON.parse(line) } catch { return }
  pending++
  handle(q).finally(() => { pending--; setImmediate(finish) }).then(r => process.stdout.write(JSON.stringify({ id: q.id, ok: true, ...r }) + '\n'),
    e => process.stdout.write(JSON.stringify({ id: q.id, ok: false, error: String(e && e.message || e) }) + '\n'))
})
rl.on('close', () => { closed = true; finish() })
