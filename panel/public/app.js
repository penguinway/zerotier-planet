'use strict';

/* ZeroTier Planet 控制台前端：无构建步骤的原生 JS 单页应用，hash 路由 */

const state = { me: null, info: null, refreshTimer: null };

const DEFAULT_RULES = `# 默认规则：只允许 IPv4、ARP、IPv6 流量
drop
\tnot ethertype ipv4
\tand not ethertype arp
\tand not ethertype ipv6
;

# 允许其余所有流量
accept;
`;

// ------------------------------------------------------------ 工具

const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

function h(s) {
  return String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

function toast(msg, isError = false) {
  const el = $('#toast');
  el.textContent = msg;
  el.className = 'show' + (isError ? ' error' : '');
  clearTimeout(toast.t);
  toast.t = setTimeout(() => { el.className = ''; }, isError ? 4000 : 2200);
}

async function api(method, path, body) {
  const opts = { method, credentials: 'same-origin', headers: {} };
  if (body !== undefined) {
    opts.headers['Content-Type'] = 'application/json';
    opts.body = JSON.stringify(body);
  }
  const res = await fetch('api/' + path, opts);
  const isJson = (res.headers.get('content-type') || '').includes('json');
  const data = isJson ? await res.json() : null;
  if (!res.ok) {
    const msg = (data && data.error) || `${res.status} ${res.statusText}`;
    if (res.status === 401 && path !== 'login') {
      state.me = null;
      go('#/login');
    } else if (res.status === 403 && msg.includes('修改初始密码')) {
      go('#/password');
    }
    throw new Error(msg);
  }
  return data;
}

// 执行异步操作：按钮防重复点击 + 统一错误提示
async function run(btn, fn, okMsg) {
  if (btn) {
    if (btn.disabled) return;
    btn.disabled = true;
  }
  try {
    const r = await fn();
    if (okMsg) toast(okMsg);
    return r;
  } catch (e) {
    toast(e.message, true);
  } finally {
    if (btn) btn.disabled = false;
  }
}

function go(hash) {
  if (location.hash === hash) route();
  else location.hash = hash;
}

function qrSvg(text) {
  const qr = qrcode(0, 'M');
  qr.addData(unescape(encodeURIComponent(text)));
  qr.make();
  return qr.createSvgTag({ cellSize: 4, margin: 2, scalable: true });
}

function qrBlock(text, label) {
  return `<div class="qr-item"><div class="qr">${qrSvg(text)}</div><div class="label">${h(label)}</div></div>`;
}

function copyBtn(text, label = '复制') {
  return `<button class="btn sm" data-copy="${h(text)}">${h(label)}</button>`;
}

document.addEventListener('click', (e) => {
  const btn = e.target.closest('[data-copy]');
  if (!btn) return;
  const text = btn.getAttribute('data-copy');
  const done = () => toast('已复制');
  if (navigator.clipboard && window.isSecureContext) {
    navigator.clipboard.writeText(text).then(done, () => fallbackCopy(text, done));
  } else {
    fallbackCopy(text, done);
  }
});

function fallbackCopy(text, done) {
  const ta = document.createElement('textarea');
  ta.value = text;
  ta.style.position = 'fixed';
  ta.style.opacity = '0';
  document.body.appendChild(ta);
  ta.select();
  try { document.execCommand('copy'); done(); } catch (_) { toast('复制失败，请手动复制', true); }
  ta.remove();
}

function fmtTime(sec) {
  if (!sec) return '-';
  return new Date(sec * 1000).toLocaleString();
}

// ------------------------------------------------------------ 布局

function layout(active, content) {
  const admin = state.me && state.me.role === 'admin';
  const items = [
    ['overview', '#/', '⌂', '概览'],
    ['networks', '#/networks', '⧉', '网络'],
    ...(admin ? [['users', '#/users', '☺', '用户'], ['planet', '#/planet', '◎', 'Planet'], ['settings', '#/settings', '⚙', '设置']] : []),
    ['account', '#/account', '☰', '账户'],
  ];
  return `
    <header class="topbar">
      <a class="brand" href="#/">ZeroTier Planet</a>
      <nav class="nav">
        ${items.map(([k, href, ico, text]) => `<a href="${href}" class="${k === active ? 'active' : ''}"><span class="ico">${ico}</span>${text}</a>`).join('')}
      </nav>
      <div class="userbox">
        <span class="who muted">${h(state.me.username)}</span>
        ${admin ? '<span class="badge admin">管理员</span>' : ''}
      </div>
    </header>
    <main>${content}</main>`;
}

function render(html) {
  $('#app').innerHTML = html;
}

// ------------------------------------------------------------ 路由

const routes = [
  [/^#\/login$/, viewLogin, false],
  [/^#\/password$/, viewForcePassword, false],
  [/^#\/?$/, viewOverview, true],
  [/^#\/networks$/, viewNetworks, true],
  [/^#\/network\/([0-9a-f]{16})(?:\/(\w+))?$/, viewNetwork, true],
  [/^#\/users$/, viewUsers, true],
  [/^#\/planet$/, viewPlanet, true],
  [/^#\/settings$/, viewSettings, true],
  [/^#\/account$/, viewAccount, true],
];

async function route() {
  clearInterval(state.refreshTimer);
  const hash = location.hash || '#/';
  for (const [re, view, needAuth] of routes) {
    const m = hash.match(re);
    if (!m) continue;
    if (needAuth) {
      if (!state.me) {
        try {
          state.me = await api('GET', 'me');
        } catch (_) {
          return;
        }
      }
      if (state.me.mustChange) return go('#/password');
    }
    try {
      await view(...m.slice(1));
    } catch (e) {
      if (state.me) render(layout('', `<div class="card error-box">${h(e.message)}</div>`));
    }
    return;
  }
  go('#/');
}

window.addEventListener('hashchange', route);

// ------------------------------------------------------------ 登录 / 改密码

function viewLogin() {
  render(`
    <div class="login">
      <h1>ZeroTier Planet 控制台</h1>
      <form class="card" id="f">
        <label class="field"><span>用户名</span><input type="text" name="username" autocomplete="username" required autofocus></label>
        <label class="field"><span>密码</span><input type="password" name="password" autocomplete="current-password" required></label>
        <button class="btn primary" style="width:100%">登录</button>
      </form>
    </div>`);
  $('#f').onsubmit = (e) => {
    e.preventDefault();
    const fd = new FormData(e.target);
    run(e.submitter, async () => {
      state.me = await api('POST', 'login', { username: fd.get('username'), password: fd.get('password') });
      go(state.me.mustChange ? '#/password' : '#/');
    });
  };
}

function passwordForm(id) {
  return `
    <form class="card" id="${id}">
      <label class="field"><span>原密码</span><input type="password" name="old" autocomplete="current-password" required></label>
      <label class="field"><span>新密码（至少 6 位）</span><input type="password" name="new" autocomplete="new-password" minlength="6" required></label>
      <label class="field"><span>确认新密码</span><input type="password" name="new2" autocomplete="new-password" minlength="6" required></label>
      <button class="btn primary">修改密码</button>
    </form>`;
}

function bindPasswordForm(id, after) {
  $('#' + id).onsubmit = (e) => {
    e.preventDefault();
    const fd = new FormData(e.target);
    if (fd.get('new') !== fd.get('new2')) return toast('两次输入的新密码不一致', true);
    run(e.submitter, async () => {
      state.me = await api('POST', 'me/password', { old: fd.get('old'), new: fd.get('new') });
      after();
    }, '密码已修改');
  };
}

async function viewForcePassword() {
  if (!state.me) {
    try { state.me = await api('GET', 'me'); } catch (_) { return; }
  }
  if (!state.me.mustChange) return go('#/');
  render(`
    <div class="login">
      <h1>请修改初始密码</h1>
      <p class="muted">账号 <b>${h(state.me.username)}</b> 正在使用初始密码，修改后才能继续使用。</p>
      ${passwordForm('pw')}
    </div>`);
  bindPasswordForm('pw', () => go('#/'));
}

function viewAccount() {
  render(layout('account', `
    <h1>账户</h1>
    <div class="card row between">
      <div>当前用户：<b>${h(state.me.username)}</b> <span class="badge ${state.me.role === 'admin' ? 'admin' : ''}">${state.me.role === 'admin' ? '管理员' : '普通用户'}</span></div>
      <button class="btn danger" id="logout">退出登录</button>
    </div>
    <h2>修改密码</h2>
    ${passwordForm('pw')}`));
  bindPasswordForm('pw', () => $('#pw').reset());
  $('#logout').onclick = (e) => run(e.target, async () => {
    await api('POST', 'logout');
    state.me = null;
    go('#/login');
  });
}

// ------------------------------------------------------------ 概览

function joinCommands(planetUrl, nwid) {
  const join = nwid || '<网络ID>';
  return {
    linux: `curl -fsSL -o /var/lib/zerotier-one/planet "${planetUrl}" && systemctl restart zerotier-one && sleep 3 && zerotier-cli join ${join}`,
    windows: `Invoke-WebRequest "${planetUrl}" -OutFile "C:\\ProgramData\\ZeroTier\\One\\planet"; Restart-Service ZeroTierOneService; Start-Sleep 3; zerotier-cli join ${join}`,
    macos: `sudo curl -fsSL -o "/Library/Application Support/ZeroTier/One/planet" "${planetUrl}" && sudo launchctl kickstart -k system/com.zerotier.one && sleep 3 && sudo zerotier-cli join ${join}`,
  };
}

function clientGuide(info, nwid) {
  const cmd = joinCommands(info.files.planet, nwid);
  return `
    <h3>Linux</h3><pre class="code">${h(cmd.linux)}</pre>${copyBtn(cmd.linux)}
    <h3>Windows（管理员 PowerShell）</h3><pre class="code">${h(cmd.windows)}</pre>${copyBtn(cmd.windows)}
    <h3>macOS</h3><pre class="code">${h(cmd.macos)}</pre>${copyBtn(cmd.macos)}
    <h3>Android / iOS</h3>
    <p class="muted small">官方 App 不支持自定义 planet。Android 可使用支持自定义 planet 的第三方客户端（如 ZerotierFix），
    用扫码得到的下载地址导入 planet，再扫描网络 ID 二维码加入。</p>`;
}

async function viewOverview() {
  const info = state.info = await api('GET', 'info');
  const node = info.node;
  render(layout('overview', `
    <h1>概览</h1>
    <div class="grid">
      <div class="card">
        <h2>控制器节点</h2>
        ${node ? `
          <p>节点地址：<code>${h(node.address)}</code></p>
          <p>ZeroTier 版本：${h(node.version)}</p>
          <p>状态：${node.online ? '<span class="badge ok">在线</span>' : '<span class="badge warn">离线</span>'}</p>
          <p>通信端口：${h(info.ztPort)}/udp、${h(info.ztPort)}/tcp</p>`
        : '<p class="error-box">无法连接 zerotier-one</p>'}
      </div>
      <div class="card">
        <h2>Planet 文件</h2>
        <div class="row" style="align-items:flex-start">
          <div class="qr">${qrSvg(info.files.planet)}</div>
          <div class="stack" style="flex:1;min-width:180px">
            <p class="small muted">扫码或复制链接下载，替换客户端的 planet 文件后重启 ZeroTier。</p>
            <div class="row">${copyBtn(info.files.planet, '复制下载链接')}<a class="btn sm" href="api/files/planet">下载</a></div>
            ${info.files.moons.map((m) => `<div class="row"><span class="small mono">${h(m.name)}</span>${copyBtn(m.url, '复制 moon 链接')}</div>`).join('')}
          </div>
        </div>
      </div>
    </div>
    <div class="card">
      <h2>根节点（${info.roots.length}）</h2>
      ${info.roots.length ? `<table class="responsive"><thead><tr><th>名称</th><th>地址</th><th>端点</th></tr></thead><tbody>
        ${info.roots.map((r) => `<tr><td data-label="名称">${h(r.name || '-')}</td><td data-label="地址"><code>${h(r.identity.slice(0, 10))}</code></td>
          <td data-label="端点"><div>${r.stableEndpoints.map((e) => `<code>${h(e)}</code>`).join('<br>')}</div></td></tr>`).join('')}
      </tbody></table>` : '<p class="muted">planet 尚未生成</p>'}
    </div>
    <div class="card">
      <h2>客户端配置</h2>
      ${clientGuide(info)}
    </div>`));
}

// ------------------------------------------------------------ 网络列表

async function viewNetworks() {
  const [networks, info] = await Promise.all([api('GET', 'networks'), state.info ? state.info : api('GET', 'info')]);
  state.info = info;
  const admin = state.me.role === 'admin';
  const canCreate = admin || info.settings.allow_user_create_network;
  render(layout('networks', `
    <div class="row between"><h1>网络</h1></div>
    ${canCreate ? `
    <details class="card" ${networks.length ? '' : 'open'}>
      <summary><b>创建网络</b></summary>
      <form id="create" style="margin-top:12px">
        <div class="grid" style="grid-template-columns:repeat(auto-fill,minmax(220px,1fr));gap:0 14px">
          <label class="field"><span>名称</span><input type="text" name="name" required maxlength="64"></label>
          <label class="field"><span>描述（可选）</span><input type="text" name="description" maxlength="200"></label>
          <label class="field"><span>IPv4 网段（可选）</span><input type="text" name="cidr" placeholder="10.11.12.0/24"></label>
          <label class="field"><span>地址池起始（可选）</span><input type="text" name="start" placeholder="10.11.12.101"></label>
          <label class="field"><span>地址池结束（可选）</span><input type="text" name="end" placeholder="10.11.12.199"></label>
        </div>
        <button class="btn primary">创建</button>
      </form>
    </details>` : ''}
    ${networks.length ? `
    <div class="card">
      <table class="responsive">
        <thead><tr><th>网络 ID</th><th>名称</th><th>网段</th><th>成员</th>${admin ? '<th>所有者</th>' : ''}<th></th></tr></thead>
        <tbody>
        ${networks.map((n) => `
          <tr>
            <td data-label="网络 ID"><a class="mono" href="#/network/${n.id}">${n.id}</a></td>
            <td data-label="名称"><div>${h(n.name || '-')}${n.meta.description ? `<div class="small muted">${h(n.meta.description)}</div>` : ''}</div></td>
            <td data-label="网段"><div>${(n.routes || []).map((r) => `<code>${h(r.target)}</code>`).join('<br>') || '-'}</div></td>
            <td data-label="成员">${n.authorizedCount}/${n.memberCount}${n.memberCount > n.authorizedCount ? ' <span class="badge warn">待授权</span>' : ''}</td>
            ${admin ? `<td data-label="所有者">${n.meta.owner ? h(n.meta.owner) : '<span class="muted">未分配</span>'}</td>` : ''}
            <td class="actions"><a class="btn sm" href="#/network/${n.id}">管理</a></td>
          </tr>`).join('')}
        </tbody>
      </table>
    </div>` : '<div class="card empty">还没有网络</div>'}`));
  const f = $('#create');
  if (f) f.onsubmit = (e) => {
    e.preventDefault();
    const fd = Object.fromEntries(new FormData(f));
    run(e.submitter, async () => {
      const r = await api('POST', 'networks', fd);
      toast('网络已创建');
      go(`#/network/${r.id}`);
    });
  };
}

// ------------------------------------------------------------ 网络详情

async function viewNetwork(nwid, tab = 'members') {
  const data = await api('GET', `networks/${nwid}`);
  if (!state.info) state.info = await api('GET', 'info');
  const net = data.network;
  const tabs = [['members', '成员'], ['settings', '设置'], ['rules', '规则'], ['join', '加入 / 二维码']];
  render(layout('networks', `
    <div class="row" style="margin-bottom:8px"><a href="#/networks" class="small">← 网络列表</a></div>
    <h1>${h(net.name || '未命名网络')} <span class="mono muted small">${nwid}</span> ${copyBtn(nwid, '复制 ID')}</h1>
    ${data.meta.description ? `<p class="muted">${h(data.meta.description)}</p>` : ''}
    <nav class="tabs">${tabs.map(([k, t]) => `<a href="#/network/${nwid}/${k}" class="${k === tab ? 'active' : ''}">${t}</a>`).join('')}</nav>
    <div id="tab"></div>`));
  const el = $('#tab');
  if (tab === 'settings') return tabSettings(el, nwid, data);
  if (tab === 'rules') return tabRules(el, nwid, data);
  if (tab === 'join') return tabJoin(el, nwid, data);
  return tabMembers(el, nwid, data);
}

// ---------- 成员

function memberCard(nwid, m) {
  const p = m.presence || {};
  const ips = m.ipAssignments || [];
  return `
    <div class="member ${m.authorized ? '' : 'pending'}" data-mid="${m.id}">
      <div class="head">
        <span class="dot ${p.online ? 'on' : ''}" title="${p.online ? '在线' : '离线'}"></span>
        <span class="id mono">${m.id}</span>
        <div class="name"><input type="text" value="${h(m.name || '')}" placeholder="备注名称" data-act="name" maxlength="64"></div>
        <label class="check" title="授权"><span class="small">${m.authorized ? '已授权' : '未授权'}</span>
          <span class="switch"><input type="checkbox" data-act="auth" ${m.authorized ? 'checked' : ''}><span></span></span></label>
      </div>
      <div class="ips">
        ${ips.map((ip) => `<span class="chip">${h(ip)}<button data-act="rmip" data-ip="${h(ip)}" title="移除">×</button></span>`).join('')}
        <form class="ip-add" data-act="addip">
          <input type="text" placeholder="添加 IP" inputmode="decimal">
          <button class="btn sm">添加</button>
          <button class="btn sm" type="button" data-act="nextip" title="从地址池自动分配">自动</button>
        </form>
      </div>
      <div class="meta">
        <span>${p.online ? `物理地址 <span class="mono">${h(p.physicalAddress || '-')}</span>` : '离线'}</span>
        ${p.online && p.latency >= 0 ? `<span>延迟 ${h(p.latency)} ms</span>` : ''}
        ${p.version ? `<span>客户端 ${h(p.version)}</span>` : ''}
        <label class="check"><input type="checkbox" data-act="bridge" ${m.activeBridge ? 'checked' : ''}>允许桥接</label>
        <label class="check"><input type="checkbox" data-act="noauto" ${m.noAutoAssignIps ? 'checked' : ''}>不自动分配 IP</label>
        <span class="spacer"></span>
        <button class="btn sm danger" data-act="del">删除</button>
      </div>
    </div>`;
}

function tabMembers(el, nwid, data) {
  let members = data.members;
  const sorted = () => [...members].sort((a, b) => (a.authorized - b.authorized) || a.id.localeCompare(b.id));
  const draw = () => {
    const pending = members.filter((m) => !m.authorized).length;
    el.innerHTML = `
      <div class="row between" style="margin-bottom:10px">
        <div class="muted small">共 ${members.length} 台设备${pending ? `，<b style="color:var(--warn)">${pending} 台待授权</b>` : ''}</div>
        <div class="row">
          <button class="btn sm" id="refresh">刷新</button>
        </div>
      </div>
      <form class="card inline-input" id="addm" style="padding:10px">
        <input type="text" name="id" placeholder="手动添加设备 ID（10 位），提前授权" maxlength="10" pattern="[0-9a-fA-F]{10}" required>
        <button class="btn">添加并授权</button>
      </form>
      <div id="mlist">${members.length ? sorted().map((m) => memberCard(nwid, m)).join('') : '<div class="card empty">还没有设备加入此网络</div>'}</div>`;
    $('#refresh', el).onclick = (e) => run(e.target, reload);
    $('#addm', el).onsubmit = (e) => {
      e.preventDefault();
      const id = new FormData(e.target).get('id').trim().toLowerCase();
      run(e.submitter, async () => {
        await api('POST', `networks/${nwid}/members`, { id, authorized: true });
        await reload();
      }, '已添加');
    };
  };

  const reload = async () => {
    const d = await api('GET', `networks/${nwid}`);
    members = d.members;
    draw();
  };

  // 用接口返回的最新成员对象替换本地数据，只重绘该卡片
  const replace = (m, updated) => {
    updated.presence = m.presence;
    members = members.map((x) => (x.id === m.id ? updated : x));
    const card = $(`.member[data-mid="${m.id}"]`, el);
    if (card) card.outerHTML = memberCard(nwid, updated);
  };
  const patch = (m, body) => api('PATCH', `networks/${nwid}/members/${m.id}`, body).then((u) => replace(m, u));
  const find = (target) => {
    const card = target.closest('.member');
    return card && members.find((x) => x.id === card.dataset.mid);
  };

  el.addEventListener('change', (e) => {
    const m = find(e.target);
    if (!m) return;
    const act = e.target.dataset.act;
    const t = e.target;
    if (act === 'auth') run(t, () => patch(m, { authorized: t.checked }), t.checked ? '已授权' : '已取消授权');
    if (act === 'bridge') run(t, () => patch(m, { activeBridge: t.checked }), '已保存');
    if (act === 'noauto') run(t, () => patch(m, { noAutoAssignIps: t.checked }), '已保存');
    if (act === 'name') run(t, () => patch(m, { name: t.value }), '名称已保存');
  });

  el.addEventListener('submit', (e) => {
    if (e.target.dataset.act !== 'addip') return;
    e.preventDefault();
    const m = find(e.target);
    const input = $('input', e.target);
    const ip = input.value.trim();
    if (!m || !ip) return;
    // 前端也去重；后端会再做一次规范化去重，彻底避免重复 IP
    if ((m.ipAssignments || []).includes(ip)) return toast('该 IP 已存在', true);
    run(e.submitter, () => patch(m, { ipAssignments: [...(m.ipAssignments || []), ip] }), 'IP 已添加');
  });

  el.addEventListener('click', (e) => {
    const t = e.target.closest('[data-act]');
    if (!t || t.tagName === 'INPUT' || t.tagName === 'FORM') return;
    const m = find(t);
    if (!m) return;
    const act = t.dataset.act;
    if (act === 'rmip') {
      run(t, () => patch(m, { ipAssignments: (m.ipAssignments || []).filter((x) => x !== t.dataset.ip) }), 'IP 已移除');
    } else if (act === 'nextip') {
      run(t, () => api('POST', `networks/${nwid}/members/${m.id}/next-ip`).then((u) => replace(m, u)), '已自动分配');
    } else if (act === 'del') {
      if (!confirm(`确定从网络中删除设备 ${m.id}？`)) return;
      run(t, async () => {
        await api('DELETE', `networks/${nwid}/members/${m.id}`);
        members = members.filter((x) => x.id !== m.id);
        draw();
      }, '已删除');
    }
  });

  draw();
  // 定时刷新在线状态；正在编辑时跳过，避免打断输入
  state.refreshTimer = setInterval(() => {
    const a = document.activeElement;
    if (document.hidden || (a && el.contains(a) && a.tagName === 'INPUT')) return;
    reload().catch(() => {});
  }, 20000);
}

// ---------- 设置

function listEditor(id, rows, fields) {
  return `<div id="${id}">${rows.map((r) => listRow(fields, r)).join('')}</div>
    <button type="button" class="btn sm" data-add="${id}">+ 添加</button>`;
}

function listRow(fields, r = {}) {
  return `<div class="list-item">${fields.map(([k, ph]) => `<input type="text" data-k="${k}" value="${h(r[k] ?? '')}" placeholder="${h(ph)}">`).join('')}
    <button type="button" class="btn sm danger" data-rm>×</button></div>`;
}

function readList(container) {
  return $$('.list-item', container).map((row) => {
    const o = {};
    $$('input', row).forEach((i) => { o[i.dataset.k] = i.value.trim(); });
    return o;
  }).filter((o) => Object.values(o).some((v) => v));
}

function tabSettings(el, nwid, data) {
  const n = data.network;
  const admin = state.me.role === 'admin';
  const v6 = n.v6AssignMode || {};
  const routeFields = [['target', '目标网段，如 10.11.12.0/24'], ['via', '网关（可选）']];
  const poolFields = [['ipRangeStart', '起始 IP'], ['ipRangeEnd', '结束 IP']];
  el.innerHTML = `
    <form class="card" id="basic">
      <h2>基本设置</h2>
      <label class="field"><span>名称</span><input type="text" name="name" value="${h(n.name)}" maxlength="64"></label>
      <label class="field"><span>描述</span><input type="text" name="description" value="${h(data.meta.description)}" maxlength="200"></label>
      <div class="row" style="gap:20px;margin-bottom:12px">
        <label class="check"><input type="checkbox" name="private" ${n.private ? 'checked' : ''}>私有网络（需要授权）</label>
        <label class="check"><input type="checkbox" name="enableBroadcast" ${n.enableBroadcast ? 'checked' : ''}>允许广播</label>
      </div>
      <div class="grid" style="grid-template-columns:repeat(auto-fill,minmax(180px,1fr));gap:0 14px">
        <label class="field"><span>MTU</span><input type="number" name="mtu" value="${h(n.mtu)}" min="1280" max="10000"></label>
        <label class="field"><span>组播接收上限</span><input type="number" name="multicastLimit" value="${h(n.multicastLimit)}" min="0"></label>
      </div>
      <button class="btn primary">保存</button>
    </form>

    <form class="card" id="easy">
      <h2>快速设置 IPv4</h2>
      <p class="small muted">根据网段自动生成路由和地址池（会覆盖下方现有路由与地址池）。</p>
      <div class="grid" style="grid-template-columns:repeat(auto-fill,minmax(180px,1fr));gap:0 14px">
        <label class="field"><span>网段</span><input type="text" name="cidr" placeholder="10.11.12.0/24" required></label>
        <label class="field"><span>起始（可选）</span><input type="text" name="start" placeholder="10.11.12.101"></label>
        <label class="field"><span>结束（可选）</span><input type="text" name="end" placeholder="10.11.12.199"></label>
      </div>
      <button class="btn">应用</button>
    </form>

    <form class="card" id="addr">
      <h2>路由与地址分配</h2>
      <h3>路由</h3>
      ${listEditor('routes', n.routes || [], routeFields)}
      <h3>IPv4 地址池</h3>
      ${listEditor('pools', n.ipAssignmentPools || [], poolFields)}
      <h3>自动分配</h3>
      <div class="row" style="gap:20px">
        <label class="check"><input type="checkbox" name="v4zt" ${n.v4AssignMode && n.v4AssignMode.zt ? 'checked' : ''}>从地址池自动分配 IPv4</label>
        <label class="check"><input type="checkbox" name="v6zt" ${v6.zt ? 'checked' : ''}>从地址池自动分配 IPv6</label>
        <label class="check"><input type="checkbox" name="v6rfc" ${v6.rfc4193 ? 'checked' : ''}>RFC4193 (/128)</label>
        <label class="check"><input type="checkbox" name="v6plane" ${v6['6plane'] ? 'checked' : ''}>6PLANE (/80)</label>
      </div>
      <h3>DNS</h3>
      <div class="grid" style="grid-template-columns:repeat(auto-fill,minmax(220px,1fr));gap:0 14px">
        <label class="field"><span>搜索域</span><input type="text" name="dnsDomain" value="${h((n.dns && n.dns.domain) || '')}" placeholder="lab.internal"></label>
        <label class="field"><span>DNS 服务器（逗号分隔）</span><input type="text" name="dnsServers" value="${h(((n.dns && n.dns.servers) || []).join(', '))}"></label>
      </div>
      <button class="btn primary">保存</button>
    </form>

    ${admin ? `
    <form class="card" id="owner">
      <h2>所有者</h2>
      <p class="small muted">所有者可以管理此网络；普通用户只能看到自己的网络。</p>
      <div class="inline-input">
        <select name="owner">${data.users.map((u) => `<option ${u === data.meta.owner ? 'selected' : ''}>${h(u)}</option>`).join('')}</select>
        <button class="btn">转移</button>
      </div>
    </form>` : ''}

    <div class="card">
      <h2 style="color:var(--danger)">删除网络</h2>
      <p class="small muted">删除后所有设备会立即断开，且无法恢复。</p>
      <button class="btn danger solid" id="delnet">删除此网络</button>
    </div>`;

  el.addEventListener('click', (e) => {
    const add = e.target.closest('[data-add]');
    if (add) {
      const id = add.dataset.add;
      $('#' + id).insertAdjacentHTML('beforeend', listRow(id === 'routes' ? routeFields : poolFields));
    }
    if (e.target.closest('[data-rm]')) e.target.closest('.list-item').remove();
  });

  const save = (btn, body) => run(btn, async () => {
    await api('PATCH', `networks/${nwid}`, body);
    await viewNetwork(nwid, 'settings');
  }, '已保存');

  $('#basic').onsubmit = (e) => {
    e.preventDefault();
    const f = e.target;
    save(e.submitter, {
      name: f.name.value.trim(),
      description: f.description.value,
      private: f.private.checked,
      enableBroadcast: f.enableBroadcast.checked,
      mtu: Number(f.mtu.value),
      multicastLimit: Number(f.multicastLimit.value),
    });
  };

  $('#easy').onsubmit = (e) => {
    e.preventDefault();
    const fd = Object.fromEntries(new FormData(e.target));
    run(e.submitter, async () => {
      await api('POST', `networks/${nwid}/easy`, fd);
      await viewNetwork(nwid, 'settings');
    }, '已应用');
  };

  $('#addr').onsubmit = (e) => {
    e.preventDefault();
    const f = e.target;
    const routes = readList($('#routes')).map((r) => ({ target: r.target, via: r.via || null }));
    const pools = readList($('#pools'));
    const servers = f.dnsServers.value.split(/[,\s]+/).map((s) => s.trim()).filter(Boolean);
    save(e.submitter, {
      routes,
      ipAssignmentPools: pools,
      v4AssignMode: { zt: f.v4zt.checked },
      v6AssignMode: { zt: f.v6zt.checked, rfc4193: f.v6rfc.checked, '6plane': f.v6plane.checked },
      dns: { domain: f.dnsDomain.value.trim(), servers },
    });
  };

  const own = $('#owner');
  if (own) own.onsubmit = (e) => {
    e.preventDefault();
    run(e.submitter, () => api('PUT', `networks/${nwid}/owner`, { owner: own.owner.value }), '所有者已更新');
  };

  $('#delnet').onclick = (e) => {
    const input = prompt(`此操作不可恢复。请输入网络 ID 确认删除：\n${nwid}`);
    if (input !== nwid) return input !== null && toast('网络 ID 不匹配', true);
    run(e.target, async () => {
      await api('DELETE', `networks/${nwid}`);
      go('#/networks');
    }, '网络已删除');
  };
}

// ---------- 规则

// 与官方 rule-compiler/cli.js 相同的输出整理
function compileRules(src) {
  const rules = [];
  const caps = {};
  const tags = {};
  const err = window.RuleCompiler.compile(src, rules, caps, tags);
  if (err) return { error: `第 ${err[0]} 行第 ${err[1]} 列：${err[2]}` };
  const capabilities = Object.values(caps);
  const tagList = Object.values(tags).map((t) => ({ id: t.id, default: (t.default || t.default === 0) ? t.default : null }));
  return { rules, capabilities, tags: tagList };
}

function tabRules(el, nwid, data) {
  const src = data.meta.rulesSource || DEFAULT_RULES;
  el.innerHTML = `
    <div class="card">
      <div class="row between">
        <h2 style="margin:0">流量规则</h2>
        <a class="small" href="https://docs.zerotier.com/rules" target="_blank" rel="noopener">规则语法文档 ↗</a>
      </div>
      <p class="small muted">使用与官方 Central 相同的规则语言，在浏览器中用官方 rule-compiler 编译后下发到 controller。
      ${data.meta.rulesSource ? '' : '<br>当前网络还没有通过面板保存过规则源码，下面是默认规则。'}</p>
      <textarea class="code" id="src" spellcheck="false">${h(src)}</textarea>
      <div id="msg" class="small" style="margin:8px 0"></div>
      <div class="row">
        <button class="btn primary" id="save">编译并保存</button>
        <button class="btn" id="check">仅检查</button>
        <button class="btn" id="reset">恢复默认</button>
        <span class="spacer"></span>
        <button class="btn sm link" id="toggle">查看当前生效的规则 JSON</button>
      </div>
      <pre class="code" id="json" style="display:none;max-height:360px">${h(JSON.stringify(data.network.rules, null, 2))}</pre>
    </div>`;
  const ta = $('#src');
  ta.addEventListener('keydown', (e) => {
    if (e.key === 'Tab') {
      e.preventDefault();
      ta.setRangeText('\t', ta.selectionStart, ta.selectionEnd, 'end');
    }
  });
  const check = () => {
    const r = compileRules(ta.value);
    const msg = $('#msg');
    if (r.error) {
      msg.className = 'small error-box';
      msg.textContent = '编译失败：' + r.error;
      return null;
    }
    msg.className = 'small ok-box';
    msg.textContent = `编译成功：${r.rules.length} 条规则，${r.capabilities.length} 个 capability，${r.tags.length} 个 tag`;
    return r;
  };
  $('#check').onclick = check;
  $('#reset').onclick = () => { ta.value = DEFAULT_RULES; check(); };
  $('#toggle').onclick = () => { const j = $('#json'); j.style.display = j.style.display === 'none' ? 'block' : 'none'; };
  $('#save').onclick = (e) => {
    const r = check();
    if (!r) return;
    run(e.target, async () => {
      const net = await api('PUT', `networks/${nwid}/rules`, { source: ta.value, ...r });
      $('#json').textContent = JSON.stringify(net.rules, null, 2);
    }, '规则已保存并生效');
  };
}

// ---------- 加入 / 二维码

function tabJoin(el, nwid, data) {
  const info = state.info;
  const roots = info.roots.map((r) => `${r.identity.slice(0, 10)} ${r.stableEndpoints.join(',')}`).join('\n');
  const summary = `网络: ${data.network.name || ''}\n网络 ID: ${nwid}\nPlanet: ${info.files.planet}\n根节点:\n${roots}`;
  el.innerHTML = `
    <div class="card">
      <h2>扫码加入</h2>
      <p class="small muted">手机扫码即可得到网络 ID、planet 下载地址等信息，免去手动输入。</p>
      <div class="qr-wrap">
        ${qrBlock(nwid, '网络 ID：' + nwid)}
        ${qrBlock(info.files.planet, 'Planet 下载地址')}
        ${qrBlock(summary, '完整接入信息')}
      </div>
      <div class="row" style="margin-top:12px">${copyBtn(nwid, '复制网络 ID')}${copyBtn(info.files.planet, '复制 planet 链接')}${copyBtn(summary, '复制完整信息')}</div>
    </div>
    <div class="card">
      <h2>客户端命令</h2>
      <p class="small muted">替换 planet 并加入本网络；之后在“成员”页授权设备。</p>
      ${clientGuide(info, nwid)}
    </div>`;
}

// ------------------------------------------------------------ 用户管理

async function viewUsers() {
  const users = await api('GET', 'users');
  render(layout('users', `
    <h1>用户</h1>
    <p class="muted small">管理员可管理所有网络、用户和 planet；普通用户只能看到和管理自己的网络，网络之间互相隔离。</p>
    <form class="card" id="create">
      <h2>新建用户</h2>
      <div class="grid" style="grid-template-columns:repeat(auto-fill,minmax(180px,1fr));gap:0 14px">
        <label class="field"><span>用户名</span><input type="text" name="username" required pattern="[A-Za-z0-9_.\\-]{2,32}"></label>
        <label class="field"><span>初始密码</span><input type="text" name="password" required minlength="6"></label>
        <label class="field"><span>角色</span><select name="role"><option value="user">普通用户</option><option value="admin">管理员</option></select></label>
      </div>
      <p class="small muted">新用户首次登录时需修改密码。</p>
      <button class="btn primary">创建</button>
    </form>
    <div class="card">
      <table class="responsive">
        <thead><tr><th>用户名</th><th>角色</th><th>网络数</th><th>创建时间</th><th></th></tr></thead>
        <tbody>${users.map((u) => `
          <tr data-user="${h(u.username)}">
            <td data-label="用户名"><b>${h(u.username)}</b>${u.mustChange ? ' <span class="badge warn">待改密</span>' : ''}${u.username === state.me.username ? ' <span class="badge">我</span>' : ''}</td>
            <td data-label="角色"><select data-act="role" ${u.username === state.me.username ? 'disabled' : ''}>
              <option value="user" ${u.role === 'user' ? 'selected' : ''}>普通用户</option>
              <option value="admin" ${u.role === 'admin' ? 'selected' : ''}>管理员</option></select></td>
            <td data-label="网络数">${u.networks}</td>
            <td data-label="创建时间" class="small">${fmtTime(u.created)}</td>
            <td class="actions"><div class="row" style="justify-content:flex-end">
              <button class="btn sm" data-act="pw">重置密码</button>
              ${u.username === state.me.username ? '' : '<button class="btn sm danger" data-act="del">删除</button>'}
            </div></td>
          </tr>`).join('')}
        </tbody>
      </table>
    </div>`));

  $('#create').onsubmit = (e) => {
    e.preventDefault();
    const fd = Object.fromEntries(new FormData(e.target));
    run(e.submitter, async () => {
      await api('POST', 'users', fd);
      await viewUsers();
    }, '用户已创建');
  };
  const main = $('main');
  main.addEventListener('change', (e) => {
    if (e.target.dataset.act !== 'role') return;
    const name = e.target.closest('tr').dataset.user;
    run(e.target, async () => {
      await api('PATCH', `users/${encodeURIComponent(name)}`, { role: e.target.value });
    }, '角色已更新').then(() => viewUsers());
  });
  main.addEventListener('click', (e) => {
    const t = e.target.closest('[data-act]');
    if (!t || t.tagName === 'SELECT') return;
    const name = t.closest('tr').dataset.user;
    if (t.dataset.act === 'pw') {
      const pw = prompt(`为 ${name} 设置新密码（至少 6 位）：`);
      if (!pw) return;
      run(t, () => api('PATCH', `users/${encodeURIComponent(name)}`, { password: pw }), '密码已重置').then(() => viewUsers());
    } else if (t.dataset.act === 'del') {
      if (!confirm(`确定删除用户 ${name}？其名下网络将转给你。`)) return;
      run(t, async () => {
        await api('DELETE', `users/${encodeURIComponent(name)}`);
        await viewUsers();
      }, '用户已删除');
    }
  });
}

// ------------------------------------------------------------ Planet（多根节点）

async function viewPlanet() {
  const data = await api('GET', 'planet');
  let roots = data.roots.map((r) => ({ ...r }));
  const draw = () => {
    render(layout('planet', `
      <h1>Planet 根节点</h1>
      <div class="card small">
        <p>一个 planet 最多可包含 ${data.maxRoots} 个根节点。多个根节点可以互为备份、就近连接，单台服务器故障不影响组网。</p>
        <p><b>添加根节点步骤：</b>在另一台服务器上以 <code>MODE=root</code> 运行本镜像，从它的日志或 <code>dist/root.json</code> 中复制根节点信息，粘贴到下方“导入根节点”。
        保存后会重新签名生成 planet 并重启本机 zerotier-one；已使用本 planet 的客户端会自动获取更新，新客户端下载新的 planet 即可。</p>
      </div>
      <div id="roots">${roots.map((r, i) => `
        <div class="card" data-i="${i}">
          <div class="row between">
            <h2 style="margin:0"><span class="mono">${h(r.identity.slice(0, 10))}</span></h2>
            ${data.local && r.identity.startsWith(data.local) ? '<span class="badge admin">本机</span>' : `<button class="btn sm danger" data-rm="${i}">移除</button>`}
          </div>
          <label class="field" style="margin-top:10px"><span>名称</span><input type="text" data-k="name" value="${h(r.name || '')}" placeholder="例如：上海 / 香港"></label>
          <label class="field"><span>端点（每行一个，格式 IP/端口）</span><textarea data-k="eps" style="min-height:70px" class="code">${h(r.stableEndpoints.join('\n'))}</textarea></label>
          <details><summary class="small muted">身份公钥</summary><pre class="code">${h(r.identity)}</pre></details>
        </div>`).join('')}
      </div>
      ${roots.length < data.maxRoots ? `
      <div class="card">
        <h2>导入根节点</h2>
        <p class="small muted">粘贴 root.json 内容：{"identity": "...", "stableEndpoints": ["IP/端口"]}</p>
        <textarea id="import" class="code" style="min-height:90px" placeholder='{"identity":"xxxxxxxxxx:0:...","stableEndpoints":["1.2.3.4/9994"]}'></textarea>
        <button class="btn" id="add" style="margin-top:8px">添加到列表</button>
      </div>` : ''}
      <div class="row"><button class="btn primary" id="save">保存并重新生成 planet</button></div>`));

    $$('#roots [data-i]').forEach((card) => {
      const i = Number(card.dataset.i);
      $('[data-k=name]', card).oninput = (e) => { roots[i].name = e.target.value; };
      $('[data-k=eps]', card).oninput = (e) => { roots[i].stableEndpoints = e.target.value.split('\n').map((s) => s.trim()).filter(Boolean); };
    });
    $$('[data-rm]').forEach((b) => { b.onclick = () => { roots.splice(Number(b.dataset.rm), 1); draw(); }; });
    const add = $('#add');
    if (add) add.onclick = () => {
      try {
        const r = JSON.parse($('#import').value);
        if (!r.identity || !Array.isArray(r.stableEndpoints)) throw new Error('缺少 identity 或 stableEndpoints');
        roots.push({ name: r.name || '', identity: r.identity, stableEndpoints: r.stableEndpoints });
        draw();
        toast('已添加到列表，点击“保存”后生效');
      } catch (e) {
        toast('解析失败：' + e.message, true);
      }
    };
    $('#save').onclick = (e) => {
      if (!confirm('将重新生成 planet 并重启本机 zerotier-one（网络会短暂中断几秒），继续？')) return;
      run(e.target, async () => {
        await api('PUT', 'planet', { roots });
        state.info = null;
        setTimeout(viewPlanet, 3000);
      }, 'planet 已重新生成，zerotier-one 正在重启');
    };
  };
  draw();
}

// ------------------------------------------------------------ 系统设置

async function viewSettings() {
  const s = await api('GET', 'settings');
  render(layout('settings', `
    <h1>系统设置</h1>
    <form class="card" id="f">
      <label class="field"><span>文件服务对外地址（可选）</span>
        <input type="url" name="public_file_url" value="${h(s.public_file_url)}" placeholder="https://example.com/zerotier-files">
      </label>
      <p class="small muted" style="margin-top:-6px">用于生成 planet / moon 下载链接与二维码。留空时使用 http://服务器IP:文件服务端口。使用反向代理时请填写。</p>
      <label class="field"><span>面板对外地址（可选）</span>
        <input type="url" name="public_url" value="${h(s.public_url)}" placeholder="https://example.com/zerotier">
      </label>
      <label class="check" style="margin-bottom:14px"><input type="checkbox" name="allow" ${s.allow_user_create_network ? 'checked' : ''}>允许普通用户创建网络</label>
      <div><button class="btn primary">保存</button></div>
    </form>`));
  $('#f').onsubmit = (e) => {
    e.preventDefault();
    const f = e.target;
    run(e.submitter, async () => {
      await api('PATCH', 'settings', {
        publicFileUrl: f.public_file_url.value,
        publicUrl: f.public_url.value,
        allowUserCreateNetwork: f.allow.checked,
      });
      state.info = null;
    }, '设置已保存');
  };
}

route();
