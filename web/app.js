// EchoStream-AI browser client.
//
// - Captures mic audio with an AudioWorklet, downsamples to 16 kHz PCM16
//   mono and streams it over the WebSocket as binary frames.
// - Applies a client-side RMS voice-activity gate so silence is never
//   uploaded (saves Whisper calls + latency).
// - Receives streamed tokens and renders them immediately; speaks sentence
//   fragments with the Web Speech API (or plays server TTS WAV when
//   TTS_PROVIDER=groq).
// - Surfaces TTFT / KV-cache-hit metrics reported by the server.

"use strict";

const $ = (id) => document.getElementById(id);

const state = {
  ws: null,
  audioCtx: null,
  mediaStream: null,
  worklet: null,
  sourceNode: null,
  recording: false,
  busy: false,        // server is processing a turn
  playing: false,     // TTS playback in progress (mute the mic gate)
  pcmBuffer: new Int16Array(0),
  lastVoiceMs: 0,
  speaking: false,    // client VAD: currently above threshold
  sentenceBuf: "",
  ttsQueue: [],
  ttftSamples: [],
  currentAgentTurn: null,
};

// ----------------------------------------------------------------- utils --

const TARGET_RATE = 16000;
const RMS_THRESHOLD = 0.012;          // float32 scale client-side VAD
const SILENCE_MS_TO_FLUSH = 700;      // end of utterance after this silence
const SEND_CHUNK_SAMPLES = 4096;      // 256 ms @16 kHz

function nowMs() { return performance.now(); }

function el(tag, className, text) {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text !== undefined) e.textContent = text;
  return e;
}

function addTurn(kind, who, text) {
  const turn = el("div", `turn ${kind}`);
  turn.appendChild(el("div", "who", who));
  const body = el("div", "text", text || "");
  turn.appendChild(body);
  $("conversation").appendChild(turn);
  $("conversation").scrollTop = $("conversation").scrollHeight;
  return body;
}

function setStage(stage) {
  const map = {
    idle: ["idle", "pill-idle"],
    listening: ["🎤 listening", "pill-idle"],
    stt: ["🗣️ transcribing", "pill-busy"],
    retrieval: ["🔎 retrieving", "pill-busy"],
    llm: ["⚡ streaming", "pill-busy"],
    tts: ["🔊 speaking", "pill-busy"],
  };
  const [label, cls] = map[stage] || [stage, "pill-idle"];
  const node = $("stage");
  node.textContent = label;
  node.className = `pill ${cls}`;
}

function fmtMs(ms) {
  return ms >= 1000 ? `${(ms / 1000).toFixed(2)} s` : `${Math.round(ms)} ms`;
}

// ------------------------------------------------------------- websocket --

function connectWs() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/ws`);
  ws.binaryType = "arraybuffer";
  state.ws = ws;

  ws.onopen = () => {
    const pill = $("ws-status");
    pill.textContent = "WS connected";
    pill.className = "pill pill-on";
    ws.send(JSON.stringify({ type: "start" }));
  };
  ws.onclose = () => {
    const pill = $("ws-status");
    pill.textContent = "WS disconnected";
    pill.className = "pill pill-off";
    setStage("idle");
    setTimeout(connectWs, 1500);
  };
  ws.onerror = () => ws.close();
  ws.onmessage = (ev) => {
    try { handleEvent(JSON.parse(ev.data)); } catch { /* ignore */ }
  };
}

function handleEvent(msg) {
  switch (msg.type) {
    case "hello":
      $("models").textContent = `${msg.stt_model} · ${msg.llm_model} · TTS: ${msg.tts_provider}`;
      break;
    case "status":
      if (msg.stage === "listening") { state.busy = false; $("btn-flush").disabled = !state.recording; }
      else { state.busy = true; $("btn-flush").disabled = true; }
      setStage(msg.stage);
      break;
    case "transcript":
      state.currentAgentTurn = addTurn("user", "you asked", msg.text);
      break;
    case "sources":
      renderSources(msg.sources || []);
      break;
    case "ttft": {
      state.ttftSamples.push(msg.ms);
      $("m-ttft").textContent = fmtMs(msg.ms);
      const avg = state.ttftSamples.reduce((a, b) => a + b, 0) / state.ttftSamples.length;
      $("m-ttft-avg").textContent = fmtMs(avg);
      const hit = msg.prompt_tokens > 0 ? msg.cached_tokens / msg.prompt_tokens : 0;
      $("m-cache").textContent = `${Math.round(hit * 100)} % (${msg.cached_tokens}/${msg.prompt_tokens} tok)`;
      break;
    }
    case "token": {
      if (!state.currentAgentTurn) state.currentAgentTurn = addTurn("agent", "echostream", "");
      state.currentAgentTurn.textContent += msg.value;
      $("conversation").scrollTop = $("conversation").scrollHeight;
      bufferForSpeech(msg.value);
      break;
    }
    case "tts":
      playWavB64(msg.data);
      break;
    case "answer_done":
      $("m-turn").textContent = fmtMs(msg.elapsed_ms);
      $("m-sources").textContent = String(msg.sources_used);
      finalizeAgentTurn(msg.answer);
      break;
    case "error":
      addTurn("agent", "error", `⚠️ ${msg.message}`);
      break;
  }
}

function renderSources(sources) {
  const box = $("sources");
  box.textContent = "";
  $("sources-count").textContent = `(${sources.length})`;
  if (!sources.length) { box.appendChild(el("div", "muted", "No matching documents above threshold.")); return; }
  for (const s of sources) {
    const card = el("div", "source");
    card.appendChild(el("div", "head", `${s.source} · #${s.score.toFixed(3)} match`));
    card.appendChild(el("div", "snippet", s.snippet));
    box.appendChild(card);
  }
}

function finalizeAgentTurn(answer) {
  state.currentAgentTurn = null;
  flushSpeech(true);
  // Keep transcript panel populated even if tokens were missed.
  if (answer) {
    const turns = document.querySelectorAll(".turn.agent .text");
    if (turns.length && turns[turns.length - 1].textContent.trim() === "") {
      turns[turns.length - 1].textContent = answer;
    }
  }
}

// -------------------------------------------------- browser TTS pipeline --

// Streaming speech synthesis: speak complete sentences as soon as they
// arrive instead of waiting for the full answer.
function bufferForSpeech(token) {
  if (!$("chk-tts").checked) return;
  if (!("speechSynthesis" in window)) return;
  state.sentenceBuf += token;
  const m = state.sentenceBuf.match(/[.!?…](\s|$)/);
  if (m) {
    const sentence = state.sentenceBuf.slice(0, m.index + 1).trim();
    state.sentenceBuf = state.sentenceBuf.slice(m.index + 1);
    if (sentence) speakSentence(sentence);
  }
}

function flushSpeech(final) {
  if (final && state.sentenceBuf.trim()) speakSentence(state.sentenceBuf.trim());
  state.sentenceBuf = "";
}

function speakSentence(text) {
  if (!("speechSynthesis" in window)) return;
  const u = new SpeechSynthesisUtterance(text);
  u.rate = 1.06; u.pitch = 1.0;
  u.onstart = () => { state.playing = true; setStage("tts"); };
  u.onend = () => {
    state.playing = speechSynthesis.speaking;
    if (!state.playing && !state.busy) setStage("listening");
  };
  speechSynthesis.speak(u);
}

function playWavB64(b64) {
  // Server-side TTS (TTS_PROVIDER=groq): decode base64 WAV and play it.
  try {
    const bin = atob(b64);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    const blob = new Blob([bytes], { type: "audio/wav" });
    const audio = new Audio(URL.createObjectURL(blob));
    audio.onplay = () => { state.playing = true; };
    audio.onended = () => { state.playing = false; };
    audio.play().catch(() => { state.playing = false; });
  } catch { /* ignore decode errors */ }
}

// ----------------------------------------------------------- audio input --

async function startRecording() {
  if (state.recording) return;
  state.mediaStream = await navigator.mediaDevices.getUserMedia({
    audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true, autoGainControl: true },
  });
  state.audioCtx = new AudioContext();
  await state.audioCtx.audioWorklet.addModule("/worklets/recorder-worklet.js");

  state.worklet = new AudioWorkletNode(state.audioCtx, "recorder-processor");
  state.worklet.port.onmessage = (ev) => onAudioBatch(new Float32Array(ev.data));

  state.sourceNode = state.audioCtx.createMediaStreamSource(state.mediaStream);
  state.sourceNode.connect(state.worklet); // worklet is a sink (no output)

  state.recording = true;
  state.lastVoiceMs = nowMs();
  $("mic-status").textContent = "microphone live @ " + state.audioCtx.sampleRate + " Hz -> 16 kHz PCM16";
  $("btn-mic").textContent = "■ Stop listening";
  $("btn-mic").classList.add("recording");
  $("btn-flush").disabled = false;
  setStage("listening");
}

function stopRecording() {
  state.recording = false;
  try { state.sourceNode && state.sourceNode.disconnect(); } catch (e) {}
  try { state.worklet && state.worklet.disconnect(); } catch (e) {}
  try { state.audioCtx && state.audioCtx.close(); } catch (e) {}
  try { state.mediaStream && state.mediaStream.getTracks().forEach((t) => t.stop()); } catch (e) {}
  state.worklet = state.sourceNode = state.audioCtx = state.mediaStream = null;
  state.pcmBuffer = new Int16Array(0);
  state.speaking = false;
  $("mic-status").textContent = "microphone off";
  $("btn-mic").textContent = "● Start listening";
  $("btn-mic").classList.remove("recording");
  $("btn-flush").disabled = true;
}

function onAudioBatch(float32) {
  if (!state.recording || !state.ws || state.ws.readyState !== 1) return;

  // Downsample context rate -> 16 kHz with linear interpolation.
  const ratio = state.audioCtx.sampleRate / TARGET_RATE;
  const outLen = Math.floor(float32.length / ratio);
  const pcm = new Int16Array(outLen);
  for (let i = 0; i < outLen; i++) {
    const src = i * ratio;
    const i0 = Math.floor(src);
    const i1 = Math.min(i0 + 1, float32.length - 1);
    const f = src - i0;
    const sample = float32[i0] * (1 - f) + float32[i1] * f;
    pcm[i] = Math.max(-32768, Math.min(32767, Math.round(sample * 32767)));
  }

  // Append to the pending buffer.
  const merged = new Int16Array(state.pcmBuffer.length + pcm.length);
  merged.set(state.pcmBuffer); merged.set(pcm, state.pcmBuffer.length);
  state.pcmBuffer = merged;

  // Client-side VAD (float32 RMS on the downsampled frame).
  const useVad = $("chk-vad").checked;
  let rms = 0;
  for (let i = 0; i < pcm.length; i++) rms += pcm[i] * pcm[i];
  rms = Math.sqrt(rms / Math.max(1, pcm.length)) / 32768;
  const voiced = rms > RMS_THRESHOLD;
  if (voiced) { state.speaking = true; state.lastVoiceMs = nowMs(); }

  // Ship 256 ms frames while (a) VAD is off, or (b) inside/just after speech.
  while (state.pcmBuffer.length >= SEND_CHUNK_SAMPLES) {
    const chunk = state.pcmBuffer.slice(0, SEND_CHUNK_SAMPLES);
    state.pcmBuffer = state.pcmBuffer.slice(SEND_CHUNK_SAMPLES);
    const gateOpen = !useVad || (state.speaking && nowMs() - state.lastVoiceMs < 250);
    if (gateOpen && !state.busy && !state.playing) {
      state.ws.send(chunk.buffer);
    }
  }
}

// ------------------------------------------------------------ wire-up -----

$("btn-mic").addEventListener("click", async () => {
  if (state.recording) { stopRecording(); } else {
    try { await startRecording(); } catch (e) {
      alert("Microphone unavailable: " + e.message);
    }
  }
});

$("btn-flush").addEventListener("click", () => {
  if (state.ws && state.ws.readyState === 1 && !state.busy) {
    if (state.pcmBuffer.length) {
      state.ws.send(state.pcmBuffer.slice(0).buffer);
      state.pcmBuffer = new Int16Array(0);
    }
    state.ws.send(JSON.stringify({ type: "flush" }));
  }
});

connectWs();
