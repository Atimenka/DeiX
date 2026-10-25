'use strict';

const dns = require('node:dns');
const { HttpError } = require('./_lib');

const DISPOSABLE = new Set(`
mailinator.com guerrillamail.com guerrillamail.net guerrillamail.org guerrillamail.biz guerrillamail.de sharklasers.com
grr.la guerrillamailblock.com pokemail.net spam4.me 10minutemail.com 10minutemail.net 10minutemail.org 10minut.com
tempmail.com temp-mail.org temp-mail.io tempmail.net tempmailo.com tempail.com tempr.email throwawaymail.com
yopmail.com yopmail.net yopmail.fr cool.fr.nf jetable.fr.nf nospam.ze.tc nomail.xl.cx mega.zik.dj speed.1s.fr
courriel.fr.nf moncourrier.fr.nf monemail.fr.nf monmail.fr.nf trashmail.com trashmail.net trashmail.de trashmail.org
trash-mail.com trash-mail.de wegwerfmail.de wegwerfmail.net wegwerfmail.org wegwerfemail.de fakeinbox.com
getnada.com nada.email maildrop.cc mailnesia.com mailcatch.com mintemail.com mytemp.email mohmal.com
dispostable.com discard.email discardmail.com discardmail.de spamgourmet.com spamgourmet.net spambox.us
spamfree24.org spamfree24.com spamfree24.de spamfree24.eu spamfree24.info spamfree24.net mailtemp.info
emailondeck.com email-fake.com fakemail.net fakemailgenerator.com fake-mail.net crazymailing.com
dropmail.me 10mail.org 20minutemail.com 30minutemail.com minuteinbox.com luxusmail.org burnermail.io
anonbox.net anonymbox.com harakirimail.com incognitomail.com incognitomail.net mailforspam.com
mailnull.com mailscrap.com mailsac.com mailzilla.com mt2015.com mt2014.com no-spam.ws nospamfor.us
objectmail.com one-time.email owlymail.com rcpt.at receiveee.com selfdestructingmail.com
sendspamhere.com sogetthis.com spamavert.com spamday.com spamevader.com spamex.com spamhole.com
spaml.com spammotel.com spamobox.com spamspot.com supermailer.jp tafmail.com teleworm.us temporaryemail.net
temporaryemail.us temporaryforwarding.com thankyou2010.com thisisnotmyrealemail.com tmail.ws tmailinator.com
trbvm.com trialmail.de turual.com twinmail.de tyldd.com uggsrock.com upliftnow.com veryrealemail.com
vomoto.com wh4f.org whyspam.me willhackforfood.biz yepmail.net zippymail.info zoemail.org
emailtemporanea.com emailtemporanea.net emailtemporario.com.br tempinbox.com tempinbox.co.uk tempmaildemo.com
mail-temporaire.fr mailtemporaire.com mailtemporaire.fr jourrapide.com armyspy.com cuvox.de dayrep.com einrot.com
fleckens.hu gustr.com rhyta.com superrito.com teleworm.com muellmail.com
byom.de cs.email imgv.de instant-mail.de lroid.com mailbox.in.ua tempmailer.com tempmailer.de
inboxbear.com inboxalias.com 1secmail.com 1secmail.net 1secmail.org esiix.com wwjmp.com xojxe.com yoggm.com
kzccv.com qiott.com vjuum.com laafd.com txcct.com rteet.com dcctb.com gmailnator.com emailnax.com
moakt.com tmpmail.net tmpmail.org tmpeml.com tempemail.co tempemail.net trashinbox.com mailpoof.com
`.split(/\s+/).filter(Boolean));

const DNS_TIMEOUT_MS = 3000;
const HARD_DNS_ERRORS = new Set(['ENOTFOUND', 'ENODATA']);

function withTimeout(promise, ms) {
  let timer;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(Object.assign(new Error('dns timeout'), { code: 'ETIMEOUT' })), ms);
  });
  return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
}

/**
 * Лёгкая проверка адреса: домен не из списка одноразовых и у него есть
 * почтовый сервер (MX или хотя бы A-запись). Сбой самого DNS-запроса адрес
 * не отклоняет: ошибка резолвера не доказывает, что почты нет.
 */
async function checkEmailDomain(email) {
  const domain = email.slice(email.lastIndexOf('@') + 1).toLowerCase();
  if (!domain.includes('.')) throw new HttpError(400, 'Укажите корректную почту');
  const parts = domain.split('.');
  for (let i = 0; i < parts.length - 1; i += 1) {
    if (DISPOSABLE.has(parts.slice(i).join('.'))) {
      throw new HttpError(400, 'Одноразовые адреса почты не принимаются');
    }
  }
  try {
    const mx = await withTimeout(dns.promises.resolveMx(domain), DNS_TIMEOUT_MS);
    if (Array.isArray(mx) && mx.some((r) => r.exchange && r.exchange !== '.')) return;
    throw Object.assign(new Error('no mx'), { code: 'ENODATA' });
  } catch (err) {
    if (!HARD_DNS_ERRORS.has(err && err.code)) return;
  }
  try {
    const a = await withTimeout(dns.promises.resolve4(domain), DNS_TIMEOUT_MS);
    if (Array.isArray(a) && a.length) return;
  } catch (err) {
    if (!HARD_DNS_ERRORS.has(err && err.code)) return;
  }
  throw new HttpError(400, 'У этого домена нет почтового сервера. Проверьте адрес.');
}

module.exports = { checkEmailDomain };
