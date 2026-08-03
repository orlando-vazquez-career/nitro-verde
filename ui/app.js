/* NitroVerde UI — vanilla JS, sin build step (riesgo ADR #7).
 *
 * Patrón anti-pérdida §C-5 (FIX-3, SSE ANTES del snapshot):
 *   1. Al conectar: new EventSource(.../events) PRIMERO — los deltas que
 *      lleguen antes del snapshot se bufferean en `pendingDeltas` (sin
 *      render), nunca se pierden en la ventana fetch→subscribe.
 *   2. LUEGO: GET /transcript (snapshot completo) y re-render total.
 *   3. Al completar el snapshot se drena el buffer: los deltas se aplican
 *      con dedup por `seq` (los que ya venían en el snapshot se descartan).
 *   4. Ante `event: resync` (receiver atrasado, cap 256): refetch del
 *      transcript y re-render total — ningún evento se pierde en silencio.
 *   5. Si el EventSource se cortó y reconecta (`onopen` tras un `onerror`),
 *      refetch del transcript: las reconexiones SSE no replayan deltas.
 */
(function () {
  'use strict';

  var state = {
    conversationId: null,
    seenSeq: {}, // dedup por seq (snapshot + deltas SSE)
    eventSource: null,
    transcriptLoaded: false, // ¿el snapshot inicial ya se aplicó?
    pendingDeltas: [], // deltas SSE llegados antes del snapshot (FIX-3)
    hadError: false, // hubo un corte SSE: el próximo onopen refetchea (FIX-3)
    pendingFile: null, // adjunto seleccionado, esperando Enviar (T19)
    pendingPreviewUrl: null // objectURL del thumbnail, para revoke
  };

  // --- DOM ---
  var chat = document.getElementById('chat');
  var phoneInput = document.getElementById('phone');
  var btnNew = document.getElementById('btn-new-conversation');
  var messageInput = document.getElementById('message-input');
  var btnSend = document.getElementById('btn-send');
  var btnAttach = document.getElementById('btn-attach');
  var fileInput = document.getElementById('file-input');
  var attachPreview = document.getElementById('attach-preview');
  var connectionStatus = document.getElementById('connection-status');
  var faultStatus = document.getElementById('fault-status');
  var btnFault500 = document.getElementById('btn-fault-500');
  var btnFaultClear = document.getElementById('btn-fault-clear');

  // --- helpers HTTP ---
  function api(path, options) {
    return fetch(path, options).then(function (res) {
      return res.text().then(function (body) {
        var json = null;
        try { json = body ? JSON.parse(body) : null; } catch (e) { /* body no JSON */ }
        return { status: res.status, json: json };
      });
    });
  }

  function postJson(path, payload) {
    return api(path, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload)
    });
  }

  function setStatus(el, text) {
    el.textContent = text || '';
  }

  // --- render ---
  function clearChat() {
    state.seenSeq = {};
    while (chat.firstChild) chat.removeChild(chat.firstChild);
  }

  function scrollToBottom() {
    chat.scrollTop = chat.scrollHeight;
  }

  function el(tag, className, text) {
    var node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined && text !== null) node.textContent = text;
    return node;
  }

  // Burbuja base según dirección: usuario a la derecha, bot a la izquierda,
  // system centrado (§C-5).
  function bubble(direction) {
    var side = direction === 'from_user' ? 'user' : (direction === 'system' ? 'system' : 'bot');
    return el('div', 'bubble bubble-' + side);
  }

  function appendEvent(event) {
    if (!event || typeof event.seq !== 'number') return;
    if (state.seenSeq[event.seq]) return; // dedup: delta ya visto en el snapshot
    state.seenSeq[event.seq] = true;

    var b = bubble(event.direction);

    switch (event.kind) {
      case 'text':
      case 'user_text':
      case 'template':
        b.appendChild(el('p', 'bubble-text', event.text || ''));
        break;

      case 'buttons':
        b.appendChild(el('p', 'bubble-text', event.text || ''));
        renderButtons(b, event.buttons || []);
        break;

      case 'list':
        b.appendChild(el('p', 'bubble-text', event.text || ''));
        renderList(b, event.sections || []);
        break;

      case 'cta_url':
        b.appendChild(el('p', 'bubble-text', event.text || ''));
        renderCta(b, event.raw);
        break;

      case 'user_button_reply':
      case 'user_list_reply':
        b.appendChild(el('p', 'bubble-text', event.text || ''));
        b.appendChild(el('span', 'bubble-hint', '↩ respuesta de botón/lista'));
        break;

      case 'image':
        renderImage(b, event);
        break;

      case 'document':
        renderDocument(b, event);
        break;

      case 'mark_as_read':
        b.appendChild(el('p', 'bubble-hint', '✓✓ mark_as_read'));
        break;

      case 'unsupported':
      default:
        b.appendChild(el('p', 'bubble-text', event.text || '(tipo no soportado)'));
        b.appendChild(el('span', 'bubble-hint', 'kind: ' + event.kind));
        break;
    }

    chat.appendChild(b);
    scrollToBottom();
  }

  // kind=buttons: botones clickeables que POSTean button_reply (§C-1).
  function renderButtons(container, buttons) {
    var group = el('div', 'bubble-actions');
    buttons.forEach(function (btn) {
      var node = el('button', 'reply-button', btn.title);
      node.type = 'button';
      node.addEventListener('click', function () {
        sendUserInput({ kind: 'button_reply', id: btn.id, title: btn.title });
      });
      group.appendChild(node);
    });
    container.appendChild(group);
  }

  // kind=list: desplegable clickeable que POSTea list_reply (§C-1).
  function renderList(container, sections) {
    var wrapper = el('details', 'reply-list');
    wrapper.appendChild(el('summary', 'reply-list-toggle', 'Ver opciones'));
    sections.forEach(function (section) {
      if (section.title) wrapper.appendChild(el('p', 'reply-list-section', section.title));
      (section.rows || []).forEach(function (row) {
        var node = el('button', 'reply-list-row', row.title);
        node.type = 'button';
        if (row.description) node.title = row.description;
        node.addEventListener('click', function () {
          wrapper.open = false;
          sendUserInput({ kind: 'list_reply', id: row.id, title: row.title });
        });
        wrapper.appendChild(node);
      });
    });
    container.appendChild(wrapper);
  }

  // kind=cta_url: link del payload wire (raw §C-3). Allowlist de esquema:
  // solo http(s) navega — un `javascript:` (u otro esquema) NO se renderiza
  // como anchor (FIX-4).
  function renderCta(container, raw) {
    try {
      var params = raw.interactive.action.parameters;
      if (!/^https?:\/\//i.test(params.url || '')) {
        container.appendChild(el('span', 'bubble-hint', params.display_text || params.url || ''));
        return;
      }
      var link = el('a', 'reply-cta', params.display_text || params.url);
      link.href = params.url;
      link.target = '_blank';
      link.rel = 'noopener noreferrer';
      container.appendChild(link);
    } catch (e) { /* raw sin la forma esperada: se muestra solo el texto */ }
  }

  // kind=image (T19): la imagen misma desde /media/{media_id} + caption.
  function renderImage(container, event) {
    if (event.media_id) {
      var img = el('img', 'bubble-media');
      img.src = '/media/' + encodeURIComponent(event.media_id);
      img.alt = event.text || 'imagen adjunta';
      img.loading = 'lazy';
      container.appendChild(img);
    }
    // event.text es el caption o la etiqueta "[imagen]" (sin caption real).
    if (event.text && event.text !== '[imagen]') {
      container.appendChild(el('p', 'bubble-text', event.text));
    }
    if (!event.media_id) {
      container.appendChild(el('span', 'bubble-hint', '(imagen sin bytes)'));
    }
  }

  // kind=document (T19): link de descarga del PDF/archivo.
  function renderDocument(container, event) {
    if (event.media_id) {
      var link = el('a', 'reply-cta', '📄 ' + (event.text || 'documento'));
      link.href = '/media/' + encodeURIComponent(event.media_id);
      link.target = '_blank';
      link.rel = 'noopener noreferrer';
      container.appendChild(link);
    } else {
      container.appendChild(el('p', 'bubble-text', event.text || 'documento'));
      container.appendChild(el('span', 'bubble-hint', '(documento sin bytes)'));
    }
  }

  // --- snapshot + tail (§C-5, FIX-3: el tail se abre ANTES del snapshot) ---
  function loadTranscript() {
    return api('/api/conversations/' + state.conversationId + '/transcript')
      .then(function (res) {
        if (res.status !== 200 || !res.json) return;
        clearChat();
        (res.json.events || []).forEach(appendEvent);
      });
  }

  // Drena los deltas buffereados mientras cargaba el snapshot: se aplican
  // con el dedup por seq de appendEvent (los ya presentes en el snapshot se
  // descartan; los appendeados durante el fetch se renderizan).
  function flushPendingDeltas() {
    var buffered = state.pendingDeltas;
    state.pendingDeltas = [];
    buffered.forEach(appendEvent);
  }

  function connectEvents() {
    if (state.eventSource) {
      state.eventSource.close();
      state.eventSource = null;
    }
    var source = new EventSource('/api/conversations/' + state.conversationId + '/events');
    state.eventSource = source;

    source.addEventListener('message', function (e) {
      var event;
      try {
        event = JSON.parse(e.data);
      } catch (err) {
        return; // delta malformado: se ignora, el snapshot manda
      }
      // FIX-3: hasta que el snapshot inicial no se aplicó, los deltas se
      // bufferean SIN renderizar (se drenan al completar loadTranscript).
      if (!state.transcriptLoaded) {
        state.pendingDeltas.push(event);
        return;
      }
      appendEvent(event);
    });

    // ADR #1: el server detectó pérdida de deltas → refetch + re-render.
    source.addEventListener('resync', function () {
      loadTranscript();
    });

    source.onopen = function () {
      setStatus(connectionStatus, 'conectado · ' + state.conversationId);
      // FIX-3: las reconexiones NO replayan los deltas perdidos durante el
      // corte → refetch del transcript (re-render total con dedup limpio).
      if (state.hadError) {
        state.hadError = false;
        loadTranscript();
      }
    };
    source.onerror = function () {
      state.hadError = true;
      setStatus(connectionStatus, 'reconectando… · ' + state.conversationId);
    };
  }

  function connect() {
    var phone = phoneInput.value.trim().replace(/^\+/, '');
    if (!phone) {
      setStatus(connectionStatus, 'ingresá un teléfono (sin +)');
      return;
    }
    setStatus(connectionStatus, 'creando conversación…');
    btnNew.disabled = true;
    postJson('/api/conversations', { phone: phone })
      .then(function (res) {
        if (res.status !== 201 || !res.json) {
          setStatus(connectionStatus, 'error creando conversación (HTTP ' + res.status + ')');
          return;
        }
        state.conversationId = res.json.conversation_id;
        messageInput.disabled = false;
        btnSend.disabled = false;
        btnAttach.disabled = false;
        clearAttachment();
        // FIX-3 (orden anti-pérdida, §C-5): 1) abrir el tail SSE (los deltas
        // quedan buffereados), 2) snapshot, 3) drenar el buffer con dedup.
        state.transcriptLoaded = false;
        state.pendingDeltas = [];
        state.hadError = false;
        connectEvents();
        return loadTranscript().then(function () {
          state.transcriptLoaded = true;
          flushPendingDeltas();
        });
      })
      .catch(function () {
        setStatus(connectionStatus, 'sin conexión con NitroVerde');
      })
      .finally(function () {
        btnNew.disabled = false;
      });
  }

  // --- input del usuario ---
  function sendUserInput(input) {
    if (!state.conversationId) return;
    postJson('/api/conversations/' + state.conversationId + '/messages', input)
      .then(function (res) {
        if (res.status === 202) return;
        if (res.status === 502 && res.json) {
          renderWebhookTargetError(res.json.status);
          return;
        }
        renderSystemNote('error enviando input (HTTP ' + res.status + ')');
      })
      .catch(function () {
        renderSystemNote('sin conexión con NitroVerde');
      });
  }

  function renderSystemNote(text) {
    var note = el('div', 'bubble bubble-system', text);
    chat.appendChild(note);
    scrollToBottom();
  }

  // 502 del webhook target: `status` 0 significa target INALCANZABLE (error
  // de transporte: DNS/conexión/timeout, convención de nv-engine), no que el
  // bot "respondió HTTP 0" (FIX-8).
  function renderWebhookTargetError(status) {
    if (status === 0) {
      renderSystemNote('bot inalcanzable (no respondió al webhook)');
      return;
    }
    renderSystemNote('el bot respondió HTTP ' + status + ' al webhook');
  }

  function sendText() {
    var body = messageInput.value.trim();
    if (!body) return;
    messageInput.value = '';
    sendUserInput({ kind: 'text', body: body });
  }

  // --- adjuntos (T19): file picker → preview → FormData al endpoint media ---
  function stageAttachment(file) {
    if (!file) return;
    clearAttachment();
    state.pendingFile = file;
    var chip = el('div', 'attach-chip');
    if (file.type && file.type.indexOf('image/') === 0) {
      state.pendingPreviewUrl = URL.createObjectURL(file);
      var img = el('img', 'attach-thumb');
      img.src = state.pendingPreviewUrl;
      img.alt = file.name;
      chip.appendChild(img);
    } else {
      chip.appendChild(el('span', 'attach-icon', '📄'));
    }
    chip.appendChild(el('span', 'attach-name', file.name));
    var cancel = el('button', 'attach-cancel', '✕');
    cancel.type = 'button';
    cancel.title = 'Quitar adjunto';
    cancel.addEventListener('click', clearAttachment);
    chip.appendChild(cancel);
    attachPreview.appendChild(chip);
    attachPreview.hidden = false;
  }

  function clearAttachment() {
    if (state.pendingPreviewUrl) {
      URL.revokeObjectURL(state.pendingPreviewUrl);
      state.pendingPreviewUrl = null;
    }
    state.pendingFile = null;
    fileInput.value = '';
    while (attachPreview.firstChild) attachPreview.removeChild(attachPreview.firstChild);
    attachPreview.hidden = true;
  }

  // Envía el adjunto staged como multipart (file + kind + caption opcional,
  // que es el texto del input al momento de enviar). Mismo manejo de status
  // que sendUserInput.
  function sendAttachment() {
    var file = state.pendingFile;
    if (!file || !state.conversationId) return;
    var caption = messageInput.value.trim();
    var kind = file.type && file.type.indexOf('image/') === 0 ? 'image' : 'document';
    var form = new FormData();
    form.append('kind', kind);
    if (caption) form.append('caption', caption);
    form.append('file', file, file.name);
    messageInput.value = '';
    clearAttachment();
    // Sin Content-Type manual: fetch arma el boundary del multipart solo.
    api('/api/conversations/' + state.conversationId + '/media', {
      method: 'POST',
      body: form
    })
      .then(function (res) {
        if (res.status === 200) return;
        if (res.status === 502 && res.json) {
          renderWebhookTargetError(res.json.status);
          return;
        }
        renderSystemNote('error enviando adjunto (HTTP ' + res.status + ')');
      })
      .catch(function () {
        renderSystemNote('sin conexión con NitroVerde');
      });
  }

  // Punto único de envío: si hay adjunto staged va como media; si no, texto.
  function send() {
    if (state.pendingFile) {
      sendAttachment();
      return;
    }
    sendText();
  }

  // --- control de fallos (§C-4, criterio de éxito #3 sin curl) ---
  function refreshFaultStatus() {
    api('/api/faults').then(function (res) {
      if (res.status !== 200 || !res.json) return;
      var policy = res.json;
      if (policy.status) {
        setStatus(faultStatus, 'fallo activo: HTTP ' + policy.status);
      } else if (policy.delay_ms) {
        setStatus(faultStatus, 'fallo activo: delay ' + policy.delay_ms + 'ms');
      } else {
        setStatus(faultStatus, '');
      }
    });
  }

  btnFault500.addEventListener('click', function () {
    postJson('/api/faults', { status: 500 }).then(refreshFaultStatus);
  });
  btnFaultClear.addEventListener('click', function () {
    api('/api/faults', { method: 'DELETE' }).then(refreshFaultStatus);
  });

  // --- wiring ---
  btnNew.addEventListener('click', connect);
  btnSend.addEventListener('click', send);
  btnAttach.addEventListener('click', function () {
    fileInput.click();
  });
  fileInput.addEventListener('change', function () {
    stageAttachment(fileInput.files && fileInput.files[0]);
  });
  messageInput.addEventListener('keydown', function (e) {
    if (e.key === 'Enter') send();
  });
  refreshFaultStatus();
})();
