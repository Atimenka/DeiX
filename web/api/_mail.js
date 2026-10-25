'use strict';

function mailEnv() {
  return {
    resend: process.env.RESEND_API_KEY || '',
    brevo: process.env.BREVO_API_KEY || '',
    smtp2go: process.env.SMTP2GO_API_KEY || '',
    from: process.env.MAIL_FROM || '',
  };
}

function mailConfigured() {
  const env = mailEnv();
  return Boolean((env.resend || env.brevo || env.smtp2go) && env.from);
}

function parseSender(from) {
  const m = /^\s*(.*?)\s*<([^<>]+)>\s*$/.exec(from);
  return m ? { name: m[1].replace(/^"|"$/g, '') || 'DeiX OS', email: m[2] } : { name: 'DeiX OS', email: from.trim() };
}

/** Отправка письма через Resend, Brevo или SMTP2GO (HTTP API, без зависимостей). Бросает Error при отказе. */
async function sendMail({ to, subject, html, text }) {
  if (!mailConfigured()) throw new Error('mail provider is not configured');
  const env = mailEnv();
  let resp;
  if (env.resend) {
    resp = await fetch('https://api.resend.com/emails', {
      method: 'POST',
      headers: { Authorization: `Bearer ${env.resend}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({ from: env.from, to: [to], subject, html, text }),
    });
  } else if (env.smtp2go) {
    const sender = parseSender(env.from);
    resp = await fetch('https://api.smtp2go.com/v3/email/send', {
      method: 'POST',
      headers: { 'X-Smtp2go-Api-Key': env.smtp2go, 'Content-Type': 'application/json', accept: 'application/json' },
      body: JSON.stringify({ sender: `${sender.name} <${sender.email}>`, to: [to], subject, html_body: html, text_body: text }),
    });
  } else {
    resp = await fetch('https://api.brevo.com/v3/smtp/email', {
      method: 'POST',
      headers: { 'api-key': env.brevo, 'Content-Type': 'application/json', accept: 'application/json' },
      body: JSON.stringify({ sender: parseSender(env.from), to: [{ email: to }], subject, htmlContent: html, textContent: text }),
    });
  }
  if (!resp.ok) {
    let detail = '';
    try {
      detail = (await resp.text()).slice(0, 200);
    } catch {
      detail = '';
    }
    throw new Error(`mail provider answered ${resp.status} ${detail}`);
  }
  if (env.smtp2go && !env.resend) {
    let body = null;
    try {
      body = JSON.parse(await resp.text());
    } catch {
      body = null;
    }
    if (!body || !body.data || body.data.failed > 0 || body.data.succeeded < 1) throw new Error('mail provider rejected the message');
  }
}

module.exports = { mailConfigured, sendMail };
