// Copied from apex-content-kit server/rui.js (2026-11-18) for r5rust's live HUD; changes marked 'r5rust'.
// Runs Respawn's weapon_hud_v2 through tools/rui-host (host.exe) and turns the result into a draw list.
// Everything positional/colour here comes from the game: transforms from the engine's layout code, values from the
// RUI's own code; this file only decodes the asset's widget + style records (layouts: tools/rui-host/RUI_ABI.md).
const fs = require('fs')
const path = require('path')
const os = require('os')
const { execFile } = require('child_process')

const ROOT = 'C:/Code/apex/apex-content-kit' // r5rust: the kit's host.exe, uiia tables and key colours, read-only
const HOST = path.join(ROOT, 'tools', 'rui-host')
const CFG_FILE = path.join(ROOT, 'rui-host.config.json')
const DEFAULT_CFG = {
  exe: 'C:/Code/reverse-engineering/reverse-engineering-projects/targets/r5apex/r5apex_dx12.exe',
  uiDll: 'D:/R5Rust/s21/ui.dll', // stock S21 paks/Win64/ui.dll, copied 2026-11-20 (the main S21 install was removed)
  exportPrefix: 'D:/tmp-apex-kit/r5f/rui2/ui/weapon_hud_v2.',
  uiImagesS21: 'D:/tmp-apex-kit/r5f/ui_hq/uiia/ui_image', // Flowstate's ui.rpak, exported from the install so its starpak (full-res) is found
  gcardImages: 'D:/tmp-apex-kit/r5f/gcard_img/uiia/ui_image', // banner frame art: every gcard_frame_*.rpak
  liveCommon: 'D:/tmp-apex-kit/live/common_img/uiia/ui_image', // live Apex common*.rpak images (akimbo Wingman icons, newer than S21)
  liveNew: 'D:/tmp-apex-kit/live/new_img/uiia/ui_image', // live ui.rpak + the newer legends' gcard_frame paks (Axle, Sparrow)
  uiImages: path.join(process.env.USERPROFILE || '', 'Documents/ApexExtract/ui-images/uiia/ui_image') // live Apex, fallback
}
const cfg = Object.assign({}, DEFAULT_CFG, fs.existsSync(CFG_FILE) ? JSON.parse(fs.readFileSync(CFG_FILE, 'utf8')) : {})

// the game's key-colour datatable (common_early.rpak), row order = COLORID index
const KEY_COLORS_FILE = path.join(ROOT, 'public', 'data', 'key_colors.csv')
const KEY_COLORS = fs.readFileSync(KEY_COLORS_FILE, 'utf8').trim().split(/\r?\n/).slice(1).map(l => {
  const name = l.match(/^"([^"]+)"/)[1]
  const v = (l.match(/"<(\d+),(\d+),(\d+)>"/) || [0, 0, 0, 0]).slice(1).map(Number)
  return { name, rgb: v }
})
const keyColor = name => (KEY_COLORS.find(k => k.name === name) || { rgb: [255, 255, 255] }).rgb
// script SrgbToLinear (standard sRGB EOTF)
const s2l = c => c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4)
const linKey = name => keyColor(name).map(v => s2l(v / 255))
// ammo type -> GetAmmoColorByType key (sh_consts.gnut ammoColors). Types with no valid ammo ref keep the script's grey.
const AMMO_KEY = { light: 'AMMO_SMALL_COLOR', heavy: 'AMMO_LARGE_COLOR', energy: 'AMMO_ENERGY_COLOR', shotgun: 'AMMO_SHUTGUN_COLOR', sniper: 'AMMO_SNIPER_COLOR', arrows: 'AMMO_ARROWS_COLOR', legendary: 'AMMO_LEGENDARY_COLOR', mythic: 'AMMO_SUPPLY_DROP_COLOR' }
const GREY = v => [s2l(v), s2l(v), s2l(v)]
const ammoColorOf = t => AMMO_KEY[t] ? linKey(AMMO_KEY[t]) : GREY(0.5)

// SetWeaponToggleHUD( icon, modeName, numFireModes, currentFireModeIndex ) calls in cl_weapon_status.gnut
const T = 'rui/hud/weapon_toggle/'
const FIRE_MODE = {
  auto: [T + 'automatic', 1, 1], single: [T + 'single_shot', 1, 1], burst: [T + 'burst', 1, 1], bow: [T + 'single_shot_bow', 1, 1],
  choke: [T + 'choke_enabled', 2, 0], 'choke-off': [T + 'choke_disabled', 2, 1], shatter: [T + 'shatter_caps_enabled_bow', 2, 1],
  // akimbo_active: WeaponHasFireModeSelect -> SetWeaponToggleHUD( single_shot, #FIRE_MODE_SINGLE, 2, 0 )
  akimbo: [T + 'single_shot', 2, 0], 'akimbo-auto': [T + 'automatic', 2, 0]
}

// High-level loadout -> RUI args, following Flowstate's cl_weapon_status.gnut (UpdateWeaponTabs / UpdateWeaponHud).
// slot = { name, hudIcon, ammo ('light'..'arrows' | 'mythic' | other), ammoIcon, mag, magMax, reserve (number | 'inf' | 'none'),
//          reserveMax, tier (1-5, 0 = no loot tier), locked (gold-kit weapon), fireMode, mastered, masteredIcon, moreSparks,
//          attachments: { barrel|mag|sight|grip|hopup: { icon, tier } (tier 0 = the empty-slot art) | undefined (n/a) } }
function fireMode (id = 'auto') {
  if (id === 'none') return { fireModeIcon: '', numFireModes: 1, currentFireModeIndex: 1 }
  const [icon, n, i] = FIRE_MODE[id] || [id, 1, 1]
  return { fireModeIcon: icon, numFireModes: n, currentFireModeIndex: i }
}

// selected = a main weapon is in hand. Holstered / melee: cl_weapon_status.gnut UpdateHudDataForMainWeapons keeps the
// last selected primary as the active tab with isMainWeaponSelected false
function buildConfig ({ slots = [], active = 0, selected = true } = {}) {
  const cur = slots[active] || null
  const cfg = { isVisible: 1, isTitan: 0, isMainWeaponSelected: selected ? 1 : 0, weaponTabSelected: active, noMainWeaponEquiped: cur ? 0 : 1, attachmentLockPips: 0 }
  // per tab: name + ammo colour; a mythic (crate) weapon's ammo colour is its rarity colour
  slots.forEach((sl, i) => {
    cfg[`weaponNameTab${i}`] = sl ? sl.name || '' : ''
    cfg[`ammoColorTab${i}`] = !sl ? [0, 0, 0] : sl.tier === 5 ? linKey('AMMO_SUPPLY_DROP_COLOR') : ammoColorOf(sl.ammo)
  })
  if (!cur) return Object.assign(cfg, { ammoIcon: '', ammoColor: [0, 0, 0] })
  const tier = cur.tier || 0
  let rarity = GREY(0.3)
  if (tier === 1 && cur.locked) rarity = linKey('FRAME_LOOT_TIER1')
  else if (tier === 2) rarity = linKey('FRAME_LOOT_TIER2')
  else if (tier === 3) rarity = linKey('FRAME_LOOT_TIER3')
  else if (tier === 4) rarity = linKey('AMMO_LEGENDARY_COLOR')
  else if (tier === 5) rarity = linKey('AMMO_SUPPLY_DROP_COLOR')
  const ammo = tier === 5 ? rarity : ammoColorOf(cur.ammo)
  const inf = cur.reserve === 'inf'
  const reserve = typeof cur.reserve === 'number' ? cur.reserve : 0
  const reserveMax = Math.max(1, cur.reserveMax || reserve)
  const magMax = Math.max(1, cur.magMax || cur.mag || 0)
  Object.assign(cfg, {
    weaponName: cur.name || '', hudIcon: cur.hudIcon || '', ammoIcon: cur.ammoIcon || '',
    weaponTier: tier, borderDefaultColor: rarity, rarityColor: rarity,
    attachmentLockPips: cur.locked && tier !== 5 ? tier : 0,
    ammoColor: ammo, ammoGlowColor: cur.locked ? rarity : ammo,
    maxMagAmmo: magMax, clipAmmoFrac: (cur.mag || 0) / magMax,
    passiveInfiniteClips: inf ? 1 : 0,
    maxAmmo: inf ? 1 : reserveMax, remainingAmmoFrac: inf ? 1 : reserve / reserveMax,
    ammoPoolVisible: inf || cur.reserve === 'none' ? 0 : 1,
    isMastered: cur.mastered ? 1 : 0, hudIconMastered: cur.masteredIcon || '', moreSparksMastered: cur.moreSparks ? 1 : 0,
    ...fireMode(cur.fireMode)
  })
  for (const p of ['barrel', 'grip', 'mag', 'sight', 'hopup']) {
    const a = cur.attachments ? cur.attachments[p] : undefined
    cfg[`${p}Allowed`] = a ? 1 : 0
    cfg[`${p}Tier`] = a ? (a.tier || 0) : 0
    cfg[`${p}Icon`] = a ? (a.icon || '') : ''
  }
  return cfg
}

// ---- per-asset data (tools/rui-host/assets/<name>/: asset.bin, meta.json, args.json via import-asset.py) ----
const ASSETS = new Map()
function asset (name) {
  let a = ASSETS.get(name)
  if (a) return a
  // r5rust: the kit's imported RUIs, then r5rust's own (D:\R5Rust\rui\assets: crosshairs)
  const dir = [path.join(HOST, 'assets', name), path.join('D:/R5Rust/rui/assets', name)].find(d => fs.existsSync(d))
  if (!dir) throw new Error(`RUI asset ${name} not imported (tools/rui-host/import-asset.py)`)
  const meta = JSON.parse(fs.readFileSync(path.join(dir, 'meta.json'), 'utf8'))
  a = {
    name, meta, blob: fs.readFileSync(path.join(dir, 'asset.bin')),
    args: Object.fromEntries(JSON.parse(fs.readFileSync(path.join(dir, 'args.json'), 'utf8')).filter(x => x.name).map(x => [x.name, x])),
    oStyle: meta.oStyle, oJobs: meta.oJobs, jobCount: meta.jobCount,
    element: meta.element && meta.element.length ? meta.element.slice(0, 2) : [1920, 1080]
  }
  ASSETS.set(name, a)
  return a
}
// per-image layout from each uiia asset header (RSX patch, RSX_UIIA_META): logical size, the opaque-content rect the
// engine trims quads to, and the logical-uv -> content-texture map (tex = uv * scale + map)
const UIIA = new Map()
for (const f of ['uiia_meta.tsv', 'uiia_meta_frames.tsv', 'uiia_meta_live.tsv', 'uiia_meta_live_new.tsv']) {
  const p = path.join(HOST, f)
  if (!fs.existsSync(p)) continue
  for (const l of fs.readFileSync(p, 'utf8').trim().split(/\r?\n/).slice(1)) {
    const c = l.split('\t'); const n = c.slice(1).map(Number)
    const k = c[0].toUpperCase()
    if (!UIIA.has(k)) UIIA.set(k, { size: [n[0], n[1]], content: [n[2], n[3]], rect: [-n[4], -n[5], n[6], n[7]], map: [n[10], n[11], n[8], n[9]] })
  }
}
const SIZES = { 0: 28, 1: 50, 2: 30, 3: 30, 4: 48, 5: 14 }

// values come from the public API: one line per arg, so strings lose line breaks/tabs (they'd inject extra
// "offset type value" lines into the native host) and numbers must be finite and int32
const str = v => String(v).replace(/[\r\n\t\0]/g, ' ').slice(0, 256)
const num = v => { const n = Number(v); return Number.isFinite(n) ? Math.max(-1e9, Math.min(1e9, n)) : 0 }
function argLines (A, config) {
  const out = []
  for (const [name, v] of Object.entries(config)) {
    const a = A.args[name]
    if (!a || v == null) continue
    const o = a.off
    switch (a.type) {
      case 'STRING': case 'IMAGE': case 'ASSET': out.push(`${o} s ${str(v)}`); break
      case 'BOOL': out.push(`${o} b ${v ? 1 : 0}`); break
      case 'INT': case 'UIHANDLE': out.push(`${o} i ${Math.round(num(v))}`); break
      case 'FLOAT': case 'GAMETIME': out.push(`${o} f ${num(v)}`); break
      case 'FLOAT3': { const a3 = Array.isArray(v) ? v : []; out.push(`${o} f3 ${num(a3[0])} ${num(a3[1])} ${num(a3[2])}`); break }
    }
  }
  return out.join('\n') + '\n'
}

// run one RUI. screen = the draw plane in pixels; outputs are normalised to it (default: the asset's element size)
// Results are cached by (asset, args, screen, nested sizes): a host run maps a ~300 MB memory snapshot, so an editor
// that changes one field should only re-run the RUI that field feeds.
const RUN_CACHE = new Map()
// time = the game clock (seconds) the RUI sees: animated frames/badges move with it
function runHost (name, config, screen, nested = '', time = 100) {
  const A = asset(name)
  screen = Array.isArray(screen) && screen.length === 2 ? screen.map(v => Math.round(Math.max(64, Math.min(7680, num(v) || 64)))) : A.element
  const key = name + '|' + argLines(A, config) + '|' + screen.join('x') + '|' + nested + '|' + time
  let p = RUN_CACHE.get(key)
  if (p) { RUN_CACHE.delete(key); RUN_CACHE.set(key, p); return p }
  p = runHostUncached(name, config, screen, nested, time)
  p.catch(() => RUN_CACHE.delete(key))
  RUN_CACHE.set(key, p)
  if (RUN_CACHE.size > 200) RUN_CACHE.delete(RUN_CACHE.keys().next().value)
  return p
}
// Persistent hosts: one `host.exe --serve` per snapshot family (assets exported in the same RSX run share addresses),
// with every imported asset of that family preloaded. Requests are queued per process; ~100 ms instead of ~2 s.
const { spawn } = require('child_process')
const POOL = new Map()
function familyOf (A) { return path.dirname(A.meta.exportPrefix) }
function serveHost (A) {
  const fam = familyOf(A)
  let h = POOL.get(fam)
  if (h && !h.dead) return h
  // the whole export folder: the host maps every snapshot in it, imported or not (an RSX_SNAPSHOT_DEDUP export writes
  // each shared memory region into whichever asset's snapshot came first, so the union needs all of them)
  const proc = spawn(path.join(HOST, 'bin', 'host.exe'), [cfg.exe, cfg.uiDll, '--serve', KEY_COLORS_FILE, fam], { windowsHide: true })
  h = { proc, queue: [], busy: null, buf: '', err: '', dead: false, ready: false }
  proc.stdout.on('data', d => {
    h.buf += d
    let i
    while ((i = h.buf.indexOf('\n')) >= 0) {
      const line = h.buf.slice(0, i).trim(); h.buf = h.buf.slice(i + 1)
      if (line === 'READY') { h.ready = true; pump(h); continue }
      const job = h.busy
      if (!job) continue
      h.busy = null
      const stderr = h.err; h.err = ''
      if (line.startsWith('DONE')) job.done(stderr)
      else job.fail(new Error(`host (${job.name}): ${line} ${stderr}`))
      pump(h)
    }
  })
  proc.stderr.on('data', d => { h.err += d; if (h.err.length > 20000) h.err = h.err.slice(-20000) })
  proc.on('exit', code => {
    h.dead = true
    const pending = [h.busy, ...h.queue].filter(Boolean)
    h.busy = null; h.queue = []
    for (const j of pending) j.fail(new Error(`host exited (${code}): ${h.err.slice(-2000)}`))
  })
  POOL.set(fam, h)
  return h
}
function pump (h) {
  if (!h.ready || h.busy || !h.queue.length || h.dead) return
  h.busy = h.queue.shift()
  h.proc.stdin.write(h.busy.line + '\n')
}

function runHostUncached (name, config, screen, nested, time = 100) {
  const A = asset(name)
  return new Promise((resolve, reject) => {
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'rui-'))
    const argsFile = path.join(tmp, 'args.txt')
    const outFile = path.join(tmp, 'out.json')
    fs.writeFileSync(argsFile, argLines(A, config))
    const cleanup = () => fs.rmSync(tmp, { recursive: true, force: true })
    const h = serveHost(A)
    const timer = setTimeout(() => { job.fail(new Error(`host timeout (${name})`)); try { h.proc.kill() } catch {} }, 30000)
    const job = {
      name,
      line: [name, A.meta.exportPrefix, argsFile, outFile, screen[0], screen[1], nested || '', time].join('\t'),
      done (stderr) {
        clearTimeout(timer)
        try {
          const j = JSON.parse(fs.readFileSync(outFile, 'utf8'))
          const data = fs.readFileSync(outFile + '.data.bin')
          j.stderr = stderr
          resolve({ j, data, A, screen })
        } catch (e) { reject(e) } finally { cleanup() }
      },
      fail (e) { clearTimeout(timer); cleanup(); reject(e) }
    }
    // public site: don't let a burst of requests pile up minutes of work behind one host
    if (h.queue.length >= 400) { job.fail(Object.assign(new Error('busy, try again in a moment'), { status: 503 })); return }
    h.queue.push(job)
    pump(h)
  })
}

const f32 = (b, o) => b.readFloatLE(o)
const i32 = (b, o) => b.readInt32LE(o)
const u16 = (b, o) => b.readUInt16LE(o)

function style (A, data, idx) {
  const B = A.blob
  const s = A.oStyle + idx * 68
  const offs = []
  for (let k = 0; k < 33; k++) offs.push(u16(B, s + 2 + k * 2))
  const col = k => [0, 1, 2, 3].map(i => f32(data, offs[k + i]))
  const F = k => f32(data, offs[k])
  return {
    type: u16(B, s),
    color: [col(0), col(4), col(8)],
    tint: col(12),
    blend: F(16), premul: F(17), hue: F(18), sat: F(19), light: F(20),
    font: u16(data, offs[21]),
    shadowAlpha: F(22), shadowOffset: [F(23), F(24)], shadowBlur: F(25),
    size: F(26), aspect: F(27), outline: F(28), thicken: F(29), blur: F(30), baseline: F(31), kerning: F(32)
  }
}

function drawList ({ j, data, A }, all = false) {
  const B = A.blob
  const out = []
  let o = A.oJobs
  const img = h => (h >= 0 && h < j.assets.length) ? { path: j.assets[h], guid: j.guids[h], meta: imageMeta(j.guids[h]) } : (h === -1 ? null : { code: h })
  for (let n = 0; n < A.jobCount; n++) {
    const t = u16(B, o)
    const w = { i: n, type: t, xfrm: u16(B, o + 4), clip: u16(B, o + 6) }
    const culled = (data[u16(B, o + 2)] & 3) !== 0
    const f2 = k => [f32(data, u16(B, o + k)), f32(data, u16(B, o + k + 2))]
    if (t === 1) {
      Object.assign(w, {
        img0: img(i32(data, u16(B, o + 8))), img1: img(i32(data, u16(B, o + 10))),
        mins: f2(12), maxs: f2(16), uvMin: f2(20), uvMax: f2(24),
        maskCenter: f2(28), maskRot: f32(data, u16(B, o + 32)), maskTranslate: f2(34), maskSize: f2(38),
        flags: u16(B, o + 46), style: style(A, data, B[o + 48])
      })
    } else if (t === 0) {
      const styles = [0, 1, 2, 3].map(k => style(A, data, B[o + 8 + k]))
      Object.assign(w, {
        text: j.texts[String(o - A.oJobs)] ?? '',
        styles, style: styles[0],
        fitToWidth: f32(data, u16(B, o + 0x0E)), fitToHeight: f32(data, u16(B, o + 0x10)),
        squish: f32(data, u16(B, o + 0x12)), lineBreak: f32(data, u16(B, o + 0x14)),
        align: f32(data, u16(B, o + 0x16)), leading: f32(data, u16(B, o + 0x1A))
      })
    } else if (t === 2) {
      Object.assign(w, { img0: img(i32(data, u16(B, o + 8))), mins: f2(10), maxs: f2(14), uvMin: f2(18), uvMax: f2(22), flags: u16(B, o + 26), style: style(A, data, B[o + 28]) })
    } else if (t === 5) {
      // nested UI: +0x08 data offset of the child's handle, +0x0A opts, +0x0C style
      Object.assign(w, { handle: i32(data, u16(B, o + 8)), opts: u16(B, o + 10), style: style(A, data, B[o + 12]) })
    } else if (t === 4) {
      // camera (the 3D character capture / PIP slot): RUI_ABI 4.6. The banner composites the legend render here.
      Object.assign(w, {
        slot: i32(data, u16(B, o + 8)), clip: img(i32(data, u16(B, o + 10))),
        mins: f2(12), maxs: f2(16), uvMin: f2(20), uvMax: f2(24), opts: u16(B, o + 44), style: style(A, data, B[o + 46])
      })
    } else {
      w.style = style(A, data, B[o + 28])
    }
    // clip: a transform whose rect scissors this widget (0 = none). Kept as an axis-aligned normalised rect.
    const cx = w.clip ? j.xfrm[w.clip] : null
    if (cx) {
      const xs = [cx[4], cx[4] + cx[0], cx[4] + cx[2], cx[4] + cx[0] + cx[2]]; const ys = [cx[5], cx[5] + cx[1], cx[5] + cx[3], cx[5] + cx[1] + cx[3]]
      w.clipRect = [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)]
    }
    const x = j.xfrm[w.xfrm]
    w.grad = x ? x.slice(0, 4) : [0, 0, 0, 0]
    w.pos = x ? x.slice(4, 6) : [0, 0]
    w.size = x ? [x[8], x[10]] : [0, 0]
    const det = w.grad[0] * w.grad[3] - w.grad[1] * w.grad[2]
    w.visible = !culled && w.style.color[0][3] > 0 && det !== 0 && (t !== 0 || w.text !== '')
    if (w.visible || all) out.push(w)
    o += SIZES[t]
  }
  return out
}

// child widgets (normalised to the child's own plane) mapped into the parent's nested widget: p' = P.pos + P.grad * p
function nestInto (parent, child, tag) {
  const [a, b, c, d] = parent.grad
  const map = (x, y) => [parent.pos[0] + a * x + c * y, parent.pos[1] + b * x + d * y]
  const mapRect = r => {
    const p = [map(r[0], r[1]), map(r[2], r[1]), map(r[0], r[3]), map(r[2], r[3])]
    return [Math.min(...p.map(q => q[0])), Math.min(...p.map(q => q[1])), Math.max(...p.map(q => q[0])), Math.max(...p.map(q => q[1]))]
  }
  return child.map(w => {
    const [g0, g1, g2, g3] = w.grad
    const out = Object.assign({}, w, {
      pos: map(w.pos[0], w.pos[1]),
      grad: [a * g0 + c * g1, b * g0 + d * g1, a * g2 + c * g3, b * g2 + d * g3],
      nested: (w.nested || 0) + 1, slot: w.slot !== undefined && w.nested ? w.slot : tag
    })
    if (w.clipRect) out.clipRect = mapRect(w.clipRect)
    if (parent.clipRect) out.clipRect = out.clipRect ? [Math.max(out.clipRect[0], parent.clipRect[0]), Math.max(out.clipRect[1], parent.clipRect[1]),
      Math.min(out.clipRect[2], parent.clipRect[2]), Math.min(out.clipRect[3], parent.clipRect[3])] : parent.clipRect
    return out
  })
}

// a parent RUI plus nested children. children: { <UIHANDLE arg name>: { name, config } | null }
async function runTree (name, config, children = {}, screen, time = 100) {
  const handles = {}
  let next = 1
  const sizes = []
  for (const argName of Object.keys(children)) {
    if (!children[argName]) continue
    handles[argName] = next
    sizes.push(`${next}=${asset(children[argName].name).element.join(',')}`)
    next++
  }
  const parent = await runHost(name, Object.assign({}, config, handles), screen, sizes.join(";"), time)
  const kids = {}
  await Promise.all(Object.entries(children).map(async ([argName, ch]) => {
    if (!ch) return
    const r = await runHost(ch.name, ch.config || {}, undefined, "", time)
    kids[handles[argName]] = drawList(r)
  }))
  const out = []
  const argOf = {}
  for (const [argName, hnd] of Object.entries(handles)) argOf[hnd] = argName
  for (const w of drawList(parent, true)) {
    if (w.type === 5) {
      const kid = kids[w.handle]
      const det = w.grad[0] * w.grad[3] - w.grad[1] * w.grad[2]
      if (kid && det !== 0) out.push(...nestInto(w, kid, argOf[w.handle]))
      continue
    }
    if (w.visible) out.push(w)
  }
  return { hidden: !!parent.j.hidden, screen: parent.screen, widgets: out, log: parent.j.log, stderr: parent.j.stderr }
}

async function hud (config, screen, loadout) {
  if (loadout) config = Object.assign(buildConfig(loadout), config || {})
  const r = await runHost('weapon_hud_v2', config, screen || [1920, 1080])
  return { hidden: !!r.j.hidden, screen: r.screen, widgets: drawList(r), log: r.j.log }
}

// Flowstate's own images first (S21 ui.rpak export), live Apex for what Flowstate doesn't ship (corrupted attachments)
const IMAGE_DIRS = [cfg.gcardImages, cfg.uiImagesS21, cfg.liveCommon, cfg.liveNew, cfg.uiImages].filter(Boolean)
function imagePath (guid) {
  if (!/^[0-9A-F]{1,16}$/i.test(guid)) return null
  const name = `0x${guid.toUpperCase().replace(/^0+(?=.)/, '')}.png`
  for (const d of IMAGE_DIRS) { const p = path.join(d, name); if (fs.existsSync(p)) return p }
  return null
}
// the crop rect only applies when it describes the PNG actually served (S21 and live art can differ)
const pngSize = new Map()
function imageMeta (guid) {
  const key = String(guid).toUpperCase().replace(/^0+(?=.)/, '')
  const m = UIIA.get(key)
  if (!m) return null
  if (!pngSize.has(key)) {
    const p = imagePath(key)
    let s = null
    if (p) { const fd = fs.openSync(p, 'r'); const b = Buffer.alloc(24); fs.readSync(fd, b, 0, 24, 0); fs.closeSync(fd); s = [b.readUInt32BE(16), b.readUInt32BE(20)] }
    pngSize.set(key, s)
  }
  const s = pngSize.get(key)
  return s && s[0] === m.content[0] && s[1] === m.content[1] ? m : null
}

module.exports = { hud, imagePath, imageMeta, cfg, runHost, runTree, buildConfig, drawList, asset }
