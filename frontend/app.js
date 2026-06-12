import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open, save } from '@tauri-apps/plugin-dialog';

// --- State ---
let state = {
  config: null,
  lastSyncTime: null,
  logs: [],
  logsTotal: 0,
  logsLoadingMore: false,
  dates: [],
  selectedDate: '',
  matrixData: [],
  progressMap: {},
  isSyncing: false,
  clockInterval: null,
};

// --- DOM refs ---
const $ = (s) => document.querySelector(s);
const $$ = (s) => document.querySelectorAll(s);

const sidebar = $('#sidebar');
const toggleBtn = $('#sidebar-toggle');
const tabs = $$('.tab-btn');
const contents = $$('.tab-content');
const syncIndicator = $('#sync-indicator');
const logsBody = $('#logs-body');
const errorPanel = $('#error-detail');
const errorTrace = $('#error-trace');
const errorClose = $('#error-close');
const matrixDate = $('#matrix-date');
const matrixContainer = $('#matrix-container');
const matrixRefresh = $('#matrix-refresh');
const confirmModal = $('#confirm-modal');
const confirmMessage = $('#confirm-message');
const modalConfirm = $('#modal-confirm');
const modalCancel = $('#modal-cancel');
const containerLoadMore = $('#load-more-container');

// --- Confirm modal ---
function showConfirmModal(message) {
  return new Promise((resolve) => {
    confirmMessage.textContent = message;
    confirmModal.classList.remove('hidden');
    const cleanup = () => {
      confirmModal.classList.add('hidden');
      modalConfirm.removeEventListener('click', onConfirm);
      modalCancel.removeEventListener('click', onCancel);
    };
    const onConfirm = () => { cleanup(); resolve(true); };
    const onCancel = () => { cleanup(); resolve(false); };
    modalConfirm.addEventListener('click', onConfirm);
    modalCancel.addEventListener('click', onCancel);
  });
}

// --- Tab switching ---
tabs.forEach((btn) => {
  btn.addEventListener('click', () => {
    tabs.forEach((b) => b.classList.remove('active'));
    btn.classList.add('active');
    contents.forEach((c) => c.classList.remove('active'));
    const tab = btn.dataset.tab;
    document.getElementById(`tab-${tab}`).classList.add('active');
    if (tab === 'matrix') refreshMatrix();
    if (tab === 'dashboard') refreshLogs();
  });
});

toggleBtn.addEventListener('click', () => {
  sidebar.classList.toggle('collapsed');
  toggleBtn.textContent = sidebar.classList.contains('collapsed') ? '>' : '<';
});

// --- Helpers ---
function formatFecha(codigo) {
  if (codigo.length !== 6) return codigo;
  const yy = codigo.slice(0, 2);
  const mm = codigo.slice(2, 4);
  const dd = codigo.slice(4, 6);
  return `${dd}/${mm}/20${yy}`;
}

function escHtml(s) {
  const d = document.createElement('div');
  d.textContent = s;
  return d.innerHTML;
}

function formatTime(d) {
  if (!d) return '';
  const pad = (n) => String(n).padStart(2, '0');
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

function updateSyncIndicator() {
  const timeStr = state.lastSyncTime ? `Último sync: ${formatTime(state.lastSyncTime)}` : '';
  document.getElementById('last-sync-time').textContent = timeStr;
}

// --- Logs ---
const LOGS_PAGE_SIZE = 50;

async function refreshLogs() {
  try {
    const res = await invoke('get_logs', { limit: LOGS_PAGE_SIZE, offset: 0 });
    state.logs = res.logs;
    state.logsTotal = res.total;
    renderLogs({ append: false });
    containerLoadMore.classList.toggle('hidden', state.logs.length >= state.logsTotal);
  } catch (e) {
    console.error('get_logs:', e);
  }
}

async function loadMoreLogs() {
  if (state.logsLoadingMore) return;
  state.logsLoadingMore = true;
  try {
    const offset = state.logs.length;
    const res = await invoke('get_logs', { limit: LOGS_PAGE_SIZE, offset });
    state.logs = state.logs.concat(res.logs);
    state.logsTotal = res.total;
    renderLogs({ append: true });
    containerLoadMore.classList.toggle('hidden', state.logs.length >= state.logsTotal);
  } catch (e) {
    console.error('loadMoreLogs:', e);
  }
  state.logsLoadingMore = false;
}

function renderLogs({ append }) {
  logsBody.innerHTML = state.logs.map((log) => {
    const levelClass = log.nivel === 'ERROR' ? 'log-level-error' : log.nivel === 'WARN' ? 'log-level-warn' : 'log-level-info';
    const canal = log.canal_contexto || '-';
    const errAttr = log.nivel === 'ERROR' ? ` data-error='${escHtml(JSON.stringify(log))}'` : '';
    return `<tr>
      <td>${escHtml(log.timestamp.split(' ')[1] || log.timestamp)}</td>
      <td><span class="${levelClass}"${errAttr}>${log.nivel}</span></td>
      <td>${escHtml(log.tipo_accion)}</td>
      <td>${escHtml(log.mensaje)}</td>
      <td>${escHtml(canal)}</td>
    </tr>`;
  }).join('');

  logsBody.querySelectorAll('.log-level-error').forEach((el) => {
    el.addEventListener('click', () => {
      try {
        const data = JSON.parse(el.dataset.error);
        errorTrace.textContent = data.traza_error || 'Sin traza.';
        errorPanel.classList.remove('hidden');
      } catch (_) {}
    });
  });

  if (!append) {
    logsBody.parentElement.scrollTop = 0;
  }
}

$('#load-more-btn').addEventListener('click', loadMoreLogs);
errorClose.addEventListener('click', () => errorPanel.classList.add('hidden'));

// --- Matrix ---
async function refreshDates() {
  try {
    state.dates = await invoke('get_dates');
    matrixDate.innerHTML = state.dates.map((d) =>
      `<option value="${d}">${formatFecha(d)}</option>`
    ).join('');
    if (state.dates.length > 0) {
      state.selectedDate = state.dates[0];
      matrixDate.value = state.selectedDate;
    }
  } catch (e) {
    console.error('get_dates:', e);
  }
}

async function refreshMatrix() {
  if (!state.selectedDate && state.dates.length > 0) {
    state.selectedDate = state.dates[0];
  }
  if (!state.selectedDate) return;

  try {
    state.matrixData = await invoke('get_matrix', { date: state.selectedDate });
    renderMatrix();
  } catch (e) {
    console.error('get_matrix:', e);
  }
}

function renderMatrix() {
  const canales = state.config?.canales || [];
  const data = state.matrixData;

  let html = '<div class="matrix-header">';
  for (let h = 0; h < 24; h++) {
    html += `<span class="matrix-header-cell">${h}</span>`;
  }
  html += '</div>';

  for (const canal of canales) {
    html += '<div class="matrix-row">';
    html += `<span class="matrix-row-label" title="${escHtml(canal)}">${escHtml(canal)}</span>`;
    for (let h = 0; h < 24; h++) {
      const entry = data.find((d) => d.canal === canal && d.hora === h);
      let color = '#d1d5db';
      let estado = 'Sin datos';
      let count = 0;
      let cellClasses = 'matrix-cell';
      let cellContent = '';
      if (entry) {
        estado = entry.estado;
        count = entry.cantidad;
        color = estado === 'Verde' ? '#22c55e' :
                estado === 'Azul' ? '#3b82f6' :
                estado === 'Naranja' ? '#fb923c' : '#ef4444';
        if (estado === 'Naranja') {
          const prog = state.progressMap[`${canal}-${h}`];
          if (prog) {
            cellClasses += ' matrix-cell-copying';
            cellContent = `${Math.round((prog.current / prog.total) * 100)}%`;
          }
        }
      }
      html += `<span class="${cellClasses}" style="background:${color}"
        data-canal="${escHtml(canal)}" data-hora="${h}"
        data-estado="${escHtml(estado)}" data-count="${count}">${cellContent}</span>`;
    }
    html += '</div>';
  }

  matrixContainer.innerHTML = html;

  let tooltip = document.querySelector('.cell-tooltip');
  if (!tooltip) {
    tooltip = document.createElement('div');
    tooltip.className = 'cell-tooltip hidden';
    document.body.appendChild(tooltip);
  }

  matrixContainer.querySelectorAll('.matrix-cell').forEach((cell) => {
    cell.addEventListener('mouseenter', (e) => {
      const c = cell.dataset.canal;
      const h = cell.dataset.hora;
      const est = cell.dataset.estado;
      const cnt = cell.dataset.count;
      const prog = state.progressMap[`${c}-${h}`];
      const progText = prog ? `<br>Progreso: ${prog.current} / ${prog.total} (${Math.round((prog.current / prog.total) * 100)}%)` : '';
      tooltip.innerHTML = `Canal: ${escHtml(c)}<br>Hora: ${String(h).padStart(2, '0')}:00<br>Estado: ${escHtml(est)}<br>Archivos: ${cnt} / 3600${progText}`;
      tooltip.classList.remove('hidden');
    });
    cell.addEventListener('mousemove', (e) => {
      tooltip.style.left = (e.clientX + 12) + 'px';
      tooltip.style.top = (e.clientY + 12) + 'px';
    });
    cell.addEventListener('mouseleave', () => {
      tooltip.classList.add('hidden');
    });
  });
}

matrixDate.addEventListener('change', () => {
  state.selectedDate = matrixDate.value;
  refreshMatrix();
});

matrixRefresh.addEventListener('click', refreshMatrix);

// --- Settings ---
async function loadConfig() {
  try {
    state.config = await invoke('get_config');
    applyConfigToForm();
  } catch (e) {
    console.error('get_config:', e);
  }
}

function applyConfigToForm() {
  const c = state.config;
  if (!c) return;
  $('#set-ruta-origen').value = c.ruta_origen || '';
  $('#set-ruta-normal').value = c.ruta_copia_normal || '';
  $('#set-ruta-tar').value = c.ruta_copia_tar || '';
  $('#set-horas-desface').value = c.horas_desface;
  $('#set-dias-persistencia').value = c.dias_persistencia;
  $('#set-margen-pre').value = c.margen_pre_minutos;
  $('#set-margen-post').value = c.margen_post_minutos;
  $('#set-habilitar-normal').checked = c.habilitar_normal;
  $('#set-habilitar-tar').checked = c.habilitar_tar;
  $('#set-max-hilos').value = c.max_hilos_copia;
}

function readConfigFromForm() {
  return {
    ruta_origen: $('#set-ruta-origen').value,
    ruta_copia_normal: $('#set-ruta-normal').value,
    ruta_copia_tar: $('#set-ruta-tar').value,
    horas_desface: parseInt($('#set-horas-desface').value) || 0,
    dias_persistencia: parseInt($('#set-dias-persistencia').value) || 2,
    habilitar_normal: $('#set-habilitar-normal').checked,
    habilitar_tar: $('#set-habilitar-tar').checked,
    margen_pre_minutos: parseInt($('#set-margen-pre').value) || 0,
    margen_post_minutos: parseInt($('#set-margen-post').value) || 0,
    max_hilos_copia: parseInt($('#set-max-hilos').value) || 4,
    canales: state.config?.canales || [],
  };
}

async function pickFolder(inputId) {
  try {
    const path = await open({ directory: true, multiple: false });
    if (path) $(inputId).value = path;
  } catch (e) {
    console.error('pick_folder:', e);
  }
}

$('#pick-origen').addEventListener('click', () => pickFolder('#set-ruta-origen'));
$('#pick-normal').addEventListener('click', () => pickFolder('#set-ruta-normal'));
$('#pick-tar').addEventListener('click', () => pickFolder('#set-ruta-tar'));

$('#settings-save').addEventListener('click', async () => {
  const config = readConfigFromForm();
  try {
    await invoke('save_config', { config: JSON.stringify(config) });
    state.config = config;
    refreshLogs();
  } catch (e) {
    console.error('save_config:', e);
  }
});

$('#settings-export').addEventListener('click', async () => {
  try {
    const path = await save({ filters: [{ name: 'JSON', extensions: ['json'] }] });
    if (path) {
      const json = JSON.stringify(readConfigFromForm(), null, 2);
      await invoke('write_file', { path, content: json });
    }
  } catch (e) {
    console.error('export:', e);
  }
});

$('#settings-import').addEventListener('click', async () => {
  try {
    const path = await open({ filters: [{ name: 'JSON', extensions: ['json'] }], multiple: false });
    if (path) {
      const json = await invoke('read_file', { path });
      const config = JSON.parse(json);
      state.config = config;
      applyConfigToForm();
    }
  } catch (e) {
    console.error('import:', e);
  }
});

// --- Channels ---
async function refreshChannels() {
  if (!state.config) return;
  const list = $('#canal-list');
  const count = $('#canal-count');
  count.textContent = `Canales activos: ${state.config.canales.length}`;
  list.innerHTML = state.config.canales.map((c, i) =>
    `<div class="canal-row">
      <code>${escHtml(c)}</code>
      <button class="canal-remove" data-canal="${escHtml(c)}">x</button>
    </div>`
  ).join('');
  list.querySelectorAll('.canal-remove').forEach((btn) => {
    btn.addEventListener('click', async () => {
      const canal = btn.dataset.canal;
      const confirmed = await showConfirmModal(`Eliminar canal "${canal}"?`);
      if (!confirmed) return;
      try {
        await invoke('remove_canal', { code: canal });
        state.config.canales = state.config.canales.filter((c) => c !== canal);
        refreshChannels();
      } catch (e) {
        console.error('remove_canal:', e);
      }
    });
  });
}

$('#canal-add').addEventListener('click', async () => {
  const input = $('#new-canal-input');
  const code = input.value.trim();
  if (!code) return;
  if (state.config?.canales.includes(code)) return;
  try {
    await invoke('add_canal', { code });
    if (state.config) state.config.canales.push(code);
    input.value = '';
    refreshChannels();
  } catch (e) {
    console.error('add_canal:', e);
  }
});

$('#new-canal-input').addEventListener('keydown', (e) => {
  if (e.key === 'Enter') $('#canal-add').click();
});

// --- Sync button ---
$('#sync-now-btn').addEventListener('click', async () => {
  try {
    await invoke('trigger_sync');
  } catch (e) {
    console.error('trigger_sync:', e);
  }
});

// --- Clocks ---
function updateClocks() {
  const now = new Date();
  const offset = state.config?.horas_desface || 0;
  const remote = new Date(now.getTime() + offset * 3600000);

  const pad = (n) => String(n).padStart(2, '0');
  $('#local-time').textContent = `${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`;
  $('#local-date').textContent = `${pad(now.getDate())}/${pad(now.getMonth() + 1)}/${now.getFullYear()}`;
  $('#remote-time').textContent = `${pad(remote.getHours())}:${pad(remote.getMinutes())}:${pad(remote.getSeconds())}`;
  $('#remote-offset').textContent = `Desfase: ${offset} hrs`;
}

// --- Events ---
async function setupListeners() {
  await listen('sync-started', () => {
    state.isSyncing = true;
    syncIndicator.textContent = 'Sincronizando...';
    syncIndicator.className = 'sync-active';
  });

  await listen('sync-finished', () => {
    state.isSyncing = false;
    state.lastSyncTime = new Date();
    state.progressMap = {};
    syncIndicator.textContent = 'Esperando ciclo';
    syncIndicator.className = 'sync-idle';
    updateSyncIndicator();
    refreshMatrix();
    refreshLogs();
  });

  await listen('status-updated', (event) => {
    const { canal, fecha, hora, status, count } = event.payload;
    const key = `${canal}-${hora}`;
    delete state.progressMap[key];
    if (fecha === state.selectedDate) {
      const idx = state.matrixData.findIndex(
        (d) => d.canal === canal && d.hora === hora
      );
      if (idx >= 0) {
        state.matrixData[idx] = { canal, hora, estado: status, cantidad: count };
      } else {
        state.matrixData.push({ canal, hora, estado: status, cantidad: count });
      }
      const cell = document.querySelector(
        `.matrix-cell[data-canal="${canal}"][data-hora="${hora}"]`
      );
      if (cell) {
        const color = status === 'Verde' ? '#22c55e' :
                      status === 'Azul' ? '#3b82f6' :
                      status === 'Naranja' ? '#fb923c' : '#ef4444';
        cell.style.background = color;
        cell.dataset.estado = status;
        cell.dataset.count = count;
        cell.textContent = '';
        cell.className = 'matrix-cell';
      }
    }
  });

  await listen('file-progress', (event) => {
    const { canal, fecha, hora, current, total } = event.payload;
    const key = `${canal}-${hora}`;
    state.progressMap[key] = { current, total };
    if (fecha === state.selectedDate) {
      const cell = document.querySelector(
        `.matrix-cell[data-canal="${canal}"][data-hora="${hora}"]`
      );
      if (cell) {
        const pct = Math.round((current / total) * 100);
        cell.textContent = `${pct}%`;
        cell.className = 'matrix-cell matrix-cell-copying';
      }
    }
  });

  await listen('log-added', () => {
    refreshLogs();
  });
}

// --- Init ---
async function init() {
  await setupListeners();
  await loadConfig();
  await refreshDates();
  await refreshLogs();
  await refreshChannels();
  updateSyncIndicator();
  updateClocks();
  state.clockInterval = setInterval(updateClocks, 1000);
}

document.addEventListener('DOMContentLoaded', init);
