(() => {
  "use strict";

  const config = window.CHIMA_CODEX_GUI || {};
  const pathPrefix = config.pathPrefix || "/codex-chima-app";
  let ws = null;
  let requestId = 1;

  const nodes = {
    activityLog: document.getElementById("activityLog"),
    checkHealth: document.getElementById("checkHealth"),
    clearLog: document.getElementById("clearLog"),
    connectSocket: document.getElementById("connectSocket"),
    disconnectSocket: document.getElementById("disconnectSocket"),
    handshakeOutput: document.getElementById("handshakeOutput"),
    healthState: document.getElementById("healthState"),
    readyUrl: document.getElementById("readyUrl"),
    socketState: document.getElementById("socketState"),
    wsUrl: document.getElementById("wsUrl"),
  };

  function httpBase() {
    return `${window.location.protocol}//${window.location.host}`;
  }

  function websocketBase() {
    return window.location.protocol === "https:" ? "wss" : "ws";
  }

  function readyUrl() {
    return config.readyUrl || `${pathPrefix}/readyz`;
  }

  function socketUrl() {
    return config.wsUrl || `${websocketBase()}://${window.location.host}${pathPrefix}/ws`;
  }

  function now() {
    return new Date().toLocaleTimeString();
  }

  function log(message) {
    const current = nodes.activityLog.textContent === "Ready." ? "" : nodes.activityLog.textContent;
    nodes.activityLog.textContent = `${current}${current ? "\n" : ""}[${now()}] ${message}`;
    nodes.activityLog.scrollTop = nodes.activityLog.scrollHeight;
  }

  function setPill(node, state, text) {
    node.dataset.state = state;
    node.lastChild.textContent = ` ${text}`;
  }

  async function checkHealth() {
    const target = readyUrl();
    setPill(nodes.healthState, "warn", "Checking health");
    log(`GET ${target}`);

    try {
      const response = await fetch(target, {
        cache: "no-store",
        credentials: "same-origin",
      });
      if (response.ok) {
        setPill(nodes.healthState, "ok", "Health OK");
        log(`Health returned ${response.status}`);
      } else {
        setPill(nodes.healthState, "bad", `Health ${response.status}`);
        log(`Health failed with ${response.status}`);
      }
    } catch (error) {
      setPill(nodes.healthState, "bad", "Health failed");
      log(`Health request failed: ${error.message}`);
    }
  }

  function initializePayload() {
    return {
      jsonrpc: "2.0",
      id: requestId++,
      method: "initialize",
      params: {
        clientInfo: {
          name: "codex-chima-public-gui",
          title: "CHIMA Codex Public GUI",
          version: "0.1.0",
        },
        capabilities: null,
      },
      trace: null,
    };
  }

  function connectSocket() {
    if (ws && ws.readyState <= WebSocket.OPEN) {
      return;
    }

    const target = socketUrl();
    setPill(nodes.socketState, "warn", "Socket connecting");
    nodes.connectSocket.disabled = true;
    log(`WS ${target}`);

    ws = new WebSocket(target);

    ws.addEventListener("open", () => {
      setPill(nodes.socketState, "ok", "Socket connected");
      nodes.disconnectSocket.disabled = false;
      const payload = initializePayload();
      ws.send(JSON.stringify(payload));
      nodes.handshakeOutput.textContent = JSON.stringify(payload, null, 2);
      log("Sent initialize request");
    });

    ws.addEventListener("message", (event) => {
      nodes.handshakeOutput.textContent = event.data;
      log("Received WebSocket frame");
    });

    ws.addEventListener("error", () => {
      setPill(nodes.socketState, "bad", "Socket error");
      log("WebSocket error");
    });

    ws.addEventListener("close", (event) => {
      setPill(nodes.socketState, "idle", "Socket closed");
      nodes.connectSocket.disabled = false;
      nodes.disconnectSocket.disabled = true;
      log(`WebSocket closed code=${event.code}`);
    });
  }

  function disconnectSocket() {
    if (ws) {
      ws.close(1000, "manual disconnect");
    }
  }

  function init() {
    nodes.readyUrl.textContent = `${httpBase()}${readyUrl()}`;
    nodes.wsUrl.textContent = socketUrl();
    nodes.checkHealth.addEventListener("click", checkHealth);
    nodes.connectSocket.addEventListener("click", connectSocket);
    nodes.disconnectSocket.addEventListener("click", disconnectSocket);
    nodes.clearLog.addEventListener("click", () => {
      nodes.activityLog.textContent = "Ready.";
    });
  }

  init();
})();
