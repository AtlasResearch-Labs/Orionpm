const http = require('http');

const port = process.env.PORT || 8080;

const server = http.createServer((req, res) => {
  res.writeHead(200, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify({
    status: 'ok',
    pid: process.pid,
    port: Number(port)
  }));
});

server.listen(port, '127.0.0.1', () => {
  console.log(`[http_worker] PID ${process.pid} listening on port ${port}`);
});

process.on('SIGTERM', () => {
  console.log(`[http_worker] PID ${process.pid} received SIGTERM, closing server`);
  server.close(() => {
    process.exit(0);
  });
});

