"""Seeded generators of the larger task inputs. Standard library only: this
file runs verbatim inside the prepare Op's sandbox, which writes the files a
generator returns and checks their digest, so that megabytes of input never
sit raw in the Graph. Each generator returns (files, truth): files maps a
path under data/ to bytes, truth is the generator's own record of what the
files hold, from which the expected values are taken.
"""

from datetime import date, datetime, timedelta, timezone
import hashlib
import json
import random


def digest(files):
    """sha256 over every file's name and bytes, in name order."""
    h = hashlib.sha256()
    for name in sorted(files):
        h.update(name.encode() + b'\0' + str(len(files[name])).encode() + b'\0' + files[name])
    return h.hexdigest()


# ---------------------------------------------------------------------------
# shards: 24 CSV exports with one quirk each, or none
# ---------------------------------------------------------------------------

SHARD_LAYOUT = ['clean', 'bom', 'v2_early', 'crlf', 'null_early', 'latin1_early', 'clean',
                'qnl_early', 'dcomma_early', 'dup_early', 'total', 'clean', 'v2_late',
                'latin1_late', 'crlf_late', 'clean', 'qnl_late', 'null_late', 'dcomma_late',
                'dup_late', 'bom', 'clean', 'total', 'clean']
SHARD_OFFSETS = ['+00:00', '+08:00', '-05:00', '+05:30', '-03:00', '+09:00', '+01:00', '-07:00',
                 '+03:00', '+10:00', '-04:00', '+02:00']
ASCII_MERCHANTS = ['Northwind', 'Blue Harbor', 'Kestrel Books', 'Pine & Oak', 'Metro Fuel',
                   'Lumen Labs', 'Granite Gym', 'Tidewater Cafe', 'Orchid Spa', 'Atlas Tools',
                   'Quarry Market', 'Zeta Holdings', 'Cafe Rouge']
ACCENT_MERCHANTS = ['Café Rouge', 'Bäckerei Süd', 'Øresund Fish', 'Crêperie Lune', 'Señor Taco']
NEWLINE_MERCHANTS = ['Atelier\nNord', 'Harbor Freight\nAnnex']
BLOCKED = ['Café Rouge', 'Øresund Fish', 'Zeta Holdings']
V1 = ['event_id', 'ts', 'merchant', 'amt', 'status']
V2 = ['event_id', 'ts', 'status', 'amount_minor', 'merchant']
EMPTY = ['', 'NULL', 'N/A']
# The first line a late quirk may appear on: past several pages of reading.
LATE = 1550


def _offset_minutes(text):
    sign = -1 if text[0] == '-' else 1
    return sign * (int(text[1:3]) * 60 + int(text[4:6]))


def _major(minor, decimal):
    whole, cents = divmod(minor, 100)
    if decimal == ',':
        grouped = f'{whole:,}'.replace(',', '.')
        return f'{grouped},{cents:02d}'
    return f'{whole}.{cents:02d}'


def _csv_line(fields, delimiter):
    out = []
    for f in fields:
        if delimiter in f or '"' in f or '\n' in f:
            f = '"' + f.replace('"', '""') + '"'
        out.append(f)
    return delimiter.join(out)


def shards(seed):
    rng = random.Random(seed)
    start = datetime(2026, 10, 1, tzinfo=timezone.utc)
    burst_hour = start + timedelta(hours=37)
    sizes = [rng.randint(2600, 4000) if kind.endswith('_late') or kind == 'total'
             else rng.randint(1500, 3600) for kind in SHARD_LAYOUT]
    offsets = [rng.choice(SHARD_OFFSETS) for _ in SHARD_LAYOUT]
    offsets[0] = '+00:00'
    burst = [0] * len(SHARD_LAYOUT)
    for _ in range(600):
        burst[rng.choice([i for i, o in enumerate(offsets) if o != '+00:00'])] += 1
    ids = rng.sample(range(10_000_000, 100_000_000), sum(sizes) + sum(burst))
    files, manifest, truth = {}, [], {'shards': {}}
    seen, every_id, duplicates, hours = set(), [], 0, {}
    for index, kind in enumerate(SHARD_LAYOUT):
        name = f'shard_{index + 1:02d}.csv'
        late = kind.endswith('_late') or kind == 'total'
        decimal, delimiter = (',', ';') if kind.startswith('dcomma') else ('.', ',')
        encoding = 'latin-1' if kind.startswith('latin1') else 'utf-8'
        offset = timedelta(minutes=_offset_minutes(offsets[index]))
        n = sizes[index] + burst[index]
        moments = sorted([start + timedelta(seconds=rng.randrange(72 * 3600))
                          for _ in range(sizes[index])]
                         + [burst_hour + timedelta(seconds=rng.randrange(3600))
                            for _ in range(burst[index])])
        rows = []
        for r in range(n):
            # r is the record index; its line is about r + 2.
            early_ok = not late or r >= LATE
            pool = list(ASCII_MERCHANTS)
            if kind.startswith('latin1') and early_ok or not kind.startswith('latin1'):
                pool += ACCENT_MERCHANTS
            merchant = rng.choice(pool) if rng.random() < 0.12 else rng.choice(ASCII_MERCHANTS[:11])
            if kind.startswith('qnl') and early_ok and rng.random() < 0.006:
                merchant = rng.choice(NEWLINE_MERCHANTS)
            if rng.random() < 0.015:
                merchant = rng.choice(BLOCKED[2:] if kind == 'latin1_late' and r < LATE
                                      else BLOCKED)
            if rng.random() < 0.015:
                minor = rng.randint(100_000, 5_000_000)
            else:
                minor = rng.randint(50, 99_999)
            roll = rng.random()
            status = 'ok' if roll < 0.95 else 'failed' if roll < 0.98 else 'pending'
            rows.append({'id': f'E{ids.pop()}', 'utc': moments[r], 'merchant': merchant,
                         'minor': minor, 'status': status})
        if kind == 'dcomma_late':
            for row in rows[:LATE]:
                row['minor'] = min(row['minor'], 99_999)
            rows[rng.randint(LATE + 50, n - 50)]['minor'] = rng.randint(100_000, 900_000)
        if kind == 'dcomma_early':
            rows[rng.randint(40, 300)]['minor'] = rng.randint(100_000, 900_000)
        if kind.startswith('null'):
            first = rng.randint(60, 300) if kind == 'null_early' else rng.randint(LATE, n - 400)
            for r in [first] + rng.sample(range(first + 1, n), 30):
                rows[r]['minor'] = None
                rows[r]['marker'] = rng.choice(EMPTY)
        if kind.startswith('qnl'):
            first = rng.randint(60, 300) if kind == 'qnl_early' else rng.randint(LATE, n - 400)
            rows[first]['merchant'] = NEWLINE_MERCHANTS[0]
        if kind.startswith('dup'):
            first = rng.randint(60, 300) if kind == 'dup_early' else rng.randint(LATE, n - 400)
            for r in [first] + rng.sample(range(first + 1, n), 40):
                rows[r]['id'] = rng.choice(every_id)
        v2_at = None
        if kind == 'v2_early':
            v2_at = rng.randint(150, 350)
        elif kind == 'v2_late':
            v2_at = rng.randint(LATE, n - 300)
        lines, accepted, rejected, total = [], 0, 0, 0
        lines.append(_csv_line(V1, delimiter))
        for r, row in enumerate(rows):
            if r == v2_at:
                lines.append(_csv_line(V2, delimiter))
            local = (row['utc'] + offset).strftime('%Y-%m-%d %H:%M:%S')
            if row['minor'] is None:
                amount = row['marker']
            elif v2_at is not None and r >= v2_at:
                amount = str(row['minor'])
            else:
                amount = _major(row['minor'], decimal)
            if v2_at is not None and r >= v2_at:
                fields = [row['id'], local, row['status'], amount, row['merchant']]
            else:
                fields = [row['id'], local, row['merchant'], amount, row['status']]
            lines.append(_csv_line(fields, delimiter))
            total += row['minor'] or 0
            if row['id'] in seen:
                duplicates += 1
                rejected += 1
                continue
            seen.add(row['id'])
            every_id.append(row['id'])
            if row['minor'] is None or row['status'] != 'ok' or row['merchant'] in BLOCKED:
                rejected += 1
                continue
            accepted += 1
            hour = row['utc'].strftime('%Y-%m-%dT%H')
            hours[hour] = hours.get(hour, 0) + 1
            row['accepted'] = True
        if kind == 'total':
            lines.append(_csv_line([f'#TOTAL {name}', str(n), _major(total, decimal)], delimiter))
        if kind == 'crlf':
            text = '\r\n'.join(lines) + '\r\n'
        elif kind == 'crlf_late':
            text = '\n'.join(lines[:LATE]) + '\n' + '\r\n'.join(lines[LATE:]) + '\r\n'
        else:
            text = '\n'.join(lines) + '\n'
        data = text.encode(encoding)
        if kind == 'bom':
            data = b'\xef\xbb\xbf' + data
        files[name] = data
        manifest.append({'file': name, 'encoding': encoding, 'decimal': decimal,
                         'delimiter': delimiter, 'utc_offset': offsets[index]})
        truth['shards'][name] = {
            'accepted': accepted, 'rejected': rejected,
            'sum_minor': sum(r['minor'] for r in rows if r.get('accepted'))}
        truth.setdefault('kinds', {})[name] = kind
    files['manifest.json'] = json.dumps({'shards': manifest, 'blocked_merchants': BLOCKED},
                                        indent=1, ensure_ascii=False).encode() + b'\n'
    peak = sorted(hours.items(), key=lambda kv: (-kv[1], kv[0]))
    truth.update({'distinct_events': len(seen), 'duplicates': duplicates, 'peak_hour': peak[0][0],
                  'peak_margin': peak[0][1] - peak[1][1]})
    return files, truth


# ---------------------------------------------------------------------------
# contracts: 48 agreements from 8 templates
# ---------------------------------------------------------------------------

_ONES = ['zero', 'one', 'two', 'three', 'four', 'five', 'six', 'seven', 'eight', 'nine', 'ten',
         'eleven', 'twelve', 'thirteen', 'fourteen', 'fifteen', 'sixteen', 'seventeen',
         'eighteen', 'nineteen']
_TENS = ['', '', 'twenty', 'thirty', 'forty', 'fifty', 'sixty', 'seventy', 'eighty', 'ninety']
MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September',
          'October', 'November', 'December']


def words(n):
    """An integer below a billion in English words: 124500 is 'one hundred
    twenty-four thousand five hundred'."""
    def small(k):
        out = []
        if k >= 100:
            out.append(_ONES[k // 100] + ' hundred')
            k %= 100
        if k >= 20:
            out.append(_TENS[k // 10] + ('-' + _ONES[k % 10] if k % 10 else ''))
        elif k:
            out.append(_ONES[k])
        return ' '.join(out)
    if n == 0:
        return 'zero'
    parts = []
    for scale, name in ((1_000_000, ' million'), (1000, ' thousand'), (1, '')):
        if n >= scale:
            parts.append(small(n // scale) + name)
            n %= scale
    return ' '.join(parts)


CONTRACT_ROLES = [('Supplier', 'Customer'), ('Provider', 'Client'), ('Vendor', 'Buyer'),
                  ('Licensor', 'Licensee')]
LAWS = ['England and Wales', 'the State of New York', 'the State of Delaware',
        'the State of California', 'the Federal Republic of Germany', 'the Republic of Singapore',
        'the Province of Ontario']
CURRENCIES = {'USD': 'US Dollars', 'EUR': 'euros', 'GBP': 'pounds sterling'}
_COMPANY_A = ['Holloway', 'Brightwater', 'Kestrel', 'Northgate', 'Silverline', 'Ambermoor',
              'Cobalt', 'Fernhill', 'Granton', 'Larkspur', 'Meridian', 'Oakridge', 'Pemberton',
              'Quillon', 'Redfern', 'Stonebridge', 'Thornbury', 'Valemont', 'Whitcombe',
              'Ashdown']
_COMPANY_B = ['Analytics', 'Logistics', 'Engineering', 'Software', 'Facilities', 'Consulting',
              'Packaging', 'Instruments', 'Laboratories', 'Communications', 'Hospitality',
              'Components']
_COMPANY_C = ['Limited', 'Ltd', 'Inc.', 'GmbH', 'LLC', 'Pte. Ltd.', 'plc']
_PEOPLE = ['Priya Raman', 'Daniel Okafor', 'Ingrid Halvorsen', 'Tomas Vidal', 'Mei Lin Zhou',
           'Samuel Achterberg', 'Lucia Ferraro', 'Oliver Brandt', 'Amara Nwosu', 'Kenji Morita',
           'Helena Sokolova', 'Rafael Mendes', 'Sofia Lindqvist', 'Marcus Webb', 'Yara Haddad',
           'Felix Hartmann', 'Nadia Petrova', 'Jonas Eriksen', 'Clara Dubois', 'Arjun Mehta']
_TITLES = ['Director', 'Chief Executive Officer', 'General Counsel', 'Managing Director',
           'Chief Financial Officer', 'Head of Procurement']
_FILLER = [
    'Each party shall perform its obligations under this Agreement with reasonable skill and '
    'care and in accordance with good industry practice.',
    'Nothing in this Agreement creates a partnership or joint venture between the parties, and '
    'neither party may bind the other.',
    'Each party shall comply with all laws and regulations that apply to it in connection with '
    'this Agreement.',
    'A party that becomes aware of a matter likely to delay the performance of this Agreement '
    'shall promptly inform the other party in writing.',
    'Each party shall keep complete and accurate records of its activities under this Agreement '
    'and shall make them available to the other party on reasonable request.',
    'The headings in this Agreement are for convenience only and do not affect its '
    'interpretation.',
    'Any reference to a statute includes that statute as amended, extended or re-enacted from '
    'time to time.',
    'No failure or delay by a party in exercising any right or remedy operates as a waiver of '
    'that right or remedy.',
    'If any provision of this Agreement is found invalid or unenforceable, the remaining '
    'provisions continue in full force and effect.',
    'Each party shall maintain adequate insurance with reputable insurers in respect of its '
    'liabilities under this Agreement.',
    'Neither party is liable for any delay in performing its obligations caused by events '
    'beyond its reasonable control.',
    'The parties shall attempt in good faith to resolve any dispute through their respective '
    'senior managers before starting proceedings.',
    'A notice is deemed received two (2) Business Days after posting, or when delivered by '
    'hand to the address of the recipient.',
    'Each party shall take reasonable technical and organisational measures to protect any '
    'personal data it processes in connection with this Agreement.',
    'Intellectual property rights that exist before the Effective Date remain the property of '
    'the party that owns them.',
    'Neither party may assign or transfer any of its rights under this Agreement without the '
    'prior written consent of the other party, which shall not be unreasonably withheld.',
    'This Agreement may be executed in any number of counterparts, each of which is an '
    'original and all of which together form one agreement.',
    'A person who is not a party to this Agreement has no right to enforce any of its terms.',
    'Each party warrants that it has full power and authority to enter into this Agreement.',
    'Where a party is required to give its consent, it shall not unreasonably withhold or '
    'delay that consent.',
    'A party in breach shall remedy that breach within twenty (20) Business Days of receiving '
    'notice of it, where the breach is capable of remedy.',
    'Each party shall ensure that its employees and subcontractors comply with the security '
    'policies of the other party when on its premises.',
]
_FILLER_HEADINGS = ['Definitions and Interpretation', 'Scope of Services', 'Service Levels',
                    'Warranties', 'Intellectual Property', 'Data Protection', 'Insurance',
                    'Force Majeure', 'Assignment', 'Notices', 'Audit', 'Subcontracting',
                    'Dispute Resolution', 'Anti-Bribery', 'Records', 'Entire Agreement',
                    'Severability', 'Third Party Rights', 'Counterparts', 'Change Control',
                    'Personnel', 'Security', 'Acceptance', 'Reporting']


def _date_text(d, style):
    if style == 0:
        return d.isoformat()
    if style == 1:
        return f'{d.day} {MONTHS[d.month - 1]} {d.year}'
    if style == 2:
        return f'{MONTHS[d.month - 1]} {d.day}, {d.year}'
    return f'{d.day:02d}/{d.month:02d}/{d.year}'


def _number(n, unit):
    return f'{words(n)} ({n}) {unit}'


def _money(cents, code, rng=None, digits=None):
    major = cents // 100
    shown = major if digits is None else digits
    return f'{words(major)} {CURRENCIES[code]} ({code} {shown:,}.00)'


def _heading(style, n, title):
    roman = ['I', 'II', 'III', 'IV', 'V', 'VI', 'VII', 'VIII', 'IX', 'X', 'XI', 'XII', 'XIII',
             'XIV', 'XV', 'XVI', 'XVII', 'XVIII', 'XIX', 'XX', 'XXI', 'XXII', 'XXIII', 'XXIV',
             'XXV', 'XXVI', 'XXVII', 'XXVIII', 'XXIX', 'XXX', 'XXXI', 'XXXII', 'XXXIII']
    return [f'{n}. {title.upper()}', f'Clause {n} ({title})', f'ARTICLE {roman[n - 1]}: {title}',
            f'Section {n}. {title}'][style]


def _wrapped(text, width=76):
    lines, line = [], ''
    for word in text.split(' '):
        if line and len(line) + 1 + len(word) > width:
            lines.append(line)
            line = word
        else:
            line = f'{line} {word}' if line else word
    return lines + [line]


def contracts(seed):
    rng = random.Random(seed)
    names = set()
    while len(names) < 96:
        names.add(f'{rng.choice(_COMPANY_A)} {rng.choice(_COMPANY_B)} {rng.choice(_COMPANY_C)}')
    names = sorted(names)
    rng.shuffle(names)
    picked = rng.sample(range(48), 22)
    amended, contradicted, hyphenated = picked[:14], set(picked[14:19]), set(picked[19:])
    term_amended, fee_amended, idle_amended = set(amended[:5]), set(amended[5:10]), set(amended[10:])
    files, truth = {}, {}
    for i in range(48):
        template = i % 8
        sup_role, cust_role = CONTRACT_ROLES[template % 4]
        customer_first = template >= 4
        style = template % 4
        supplier, customer = names[2 * i], names[2 * i + 1]
        reference = f'{["MSA", "SLA", "LIC", "SUP"][template % 4]}-{2025 + i % 2}-{rng.randint(100, 9999):04d}'
        effective = date(2025, 1, 1) + timedelta(rng.randrange(700))
        signed = effective - timedelta(rng.randint(3, 40))
        date_style = rng.randrange(4)
        if date_style == 3 and effective.day > 12 and rng.random() < 0.6:
            effective = effective.replace(day=rng.randint(1, 12))
        term = rng.choice([12, 24, 36, 48, 60])
        in_years = term % 12 == 0 and rng.random() < 0.5
        code = rng.choice(list(CURRENCIES))
        fee = rng.randint(24, 3600) * 50000
        cap = rng.randint(10, 400) * 2_500_000
        shown_cap = None
        while i in contradicted and shown_cap in (None, cap // 100):
            # A slip of the pen in the figures: two digits swapped or one off.
            digits = list(str(cap // 100))
            k = rng.randrange(len(digits) - 3)
            if digits[k] == digits[k + 1]:
                digits[k + 1] = str((int(digits[k + 1]) + 3) % 10)
            else:
                digits[k], digits[k + 1] = digits[k + 1], digits[k]
            if digits[0] != '0':
                shown_cap = int(''.join(digits))
        record = {
            'reference': reference, 'supplier': supplier, 'customer': customer,
            'effective_date': effective.isoformat(), 'term_months': term,
            'auto_renew': rng.random() < 0.5, 'annual_fee_cents': fee, 'currency': code,
            'payment_days': rng.choice([14, 30, 45, 60, 90]),
            'late_interest_pct': rng.choice([2, 3, 4, 4.5, 5, 6, 8, 8.5]),
            'notice_days': rng.choice([30, 60, 90, 120, 180]),
            'liability_cap_cents': cap, 'confidentiality_years': rng.choice([2, 3, 5, 7, 10]),
            'governing_law': rng.choice(LAWS),
            'signatory_supplier': rng.choice(_PEOPLE)}
        sup_sign = record['signatory_supplier']
        cust_sign = rng.choice([p for p in _PEOPLE if p != sup_sign])

        def party(n, name, role):
            return (f'({n}) {name}, a company incorporated in {rng.choice(["England", "Delaware", "Ontario", "Singapore", "Germany"])} '
                    f'with registered number {rng.randint(1000000, 9999999)}, whose registered '
                    f'office is at {rng.randint(1, 250)} {rng.choice(["King", "Harbour", "Station", "Market", "Mill"])} '
                    f'{rng.choice(["Street", "Road", "Lane", "Quay"])} (the "{role}")')

        lines = [f'{["MASTER SERVICES AGREEMENT", "SERVICE LEVEL AGREEMENT", "LICENCE AGREEMENT", "SUPPLY AGREEMENT"][template % 4]}',
                 '', f'Agreement No. {reference}', '',
                 f'This Agreement is made on {_date_text(signed, (date_style + 1) % 4)} between:', '']
        parties = [party(1, supplier, sup_role), party(2, customer, cust_role)]
        if customer_first:
            parties = [party(1, customer, cust_role), party(2, supplier, sup_role)]
        for k, text in enumerate(parties):
            lines += _wrapped(text + ('; and' if k == 0 else '.')) + ['']
        recital = rng.sample(_FILLER, 3)
        lines += ['BACKGROUND', ''] + _wrapped(' '.join(recital)) + ['']
        term_text = _number(term // 12, 'years') if in_years else _number(term, 'months')
        renew = (f'This Agreement renews automatically for successive periods of twelve (12) '
                 f'months unless either party gives notice of non-renewal before the end of the '
                 f'current term.' if record['auto_renew'] else
                 'This Agreement does not renew automatically; any extension requires a written '
                 'agreement signed by both parties.')
        key = [
            ('Term', f'This Agreement commences on {_date_text(effective, date_style)} (the '
                     f'"Effective Date") and continues for an initial term of {term_text}.'),
            ('Renewal', renew),
            ('Fees', f'The {cust_role} shall pay the {sup_role} an annual fee of '
                     f'{_money(fee, code)}, exclusive of applicable taxes.'),
            ('Payment', f'The {cust_role} shall pay each undisputed invoice within '
                        f'{_number(record["payment_days"], "days")} of the date of the invoice.'),
            ('Late Payment', f'Overdue amounts bear interest at {record["late_interest_pct"]} '
                             f'percent per annum from the due date until paid.'),
            ('Confidentiality', f'The obligations of confidentiality in this clause survive for '
                                f'{_number(record["confidentiality_years"], "years")} after '
                                f'termination of this Agreement.'),
            ('Limitation of Liability', f'The total liability of each party under this '
                                        f'Agreement shall not exceed '
                                        f'{_money(cap, code, digits=shown_cap)}.'),
            ('Termination for Convenience', f'Either party may terminate this Agreement for '
                                            f'convenience by giving not less than '
                                            f'{_number(record["notice_days"], "days")}\' written '
                                            f'notice to the other party.'),
            ('Governing Law', f'This Agreement is governed by the laws of '
                              f'{record["governing_law"]}.'),
        ]
        # Contracts with a trap run past line 400 and hold it beyond the
        # first pages: the annex at the end, or the liability or governing
        # law clause moved last; the others run 120 to 300 lines.
        long = i in amended or i in contradicted or i in hyphenated
        target = rng.randint(445, 475) if long else rng.randint(120, 300)
        sections = [[heading, [text] + ([rng.choice(_FILLER)] if rng.random() < 0.5 else [])]
                    for heading, text in key]
        rng.shuffle(sections)
        for heading in rng.sample(_FILLER_HEADINGS, rng.randint(2, 10)):
            sections.insert(rng.randint(0, len(sections)), [heading, []])
        last = ('Limitation of Liability' if i in contradicted
                else 'Governing Law' if i in hyphenated else None)
        if last:
            moved = next(s for s in sections if s[0] == last)
            sections.remove(moved)
            sections.append(moved)
        fill = [s for s in sections if not s[1]]

        def render():
            out = []
            for n, (heading, paragraphs) in enumerate(sections, start=1):
                out += [_heading(style, n, heading), '']
                for p, text in enumerate(paragraphs, start=1):
                    out += _wrapped(f'{n}.{p} {text}') + ['']
            return out

        rest = target - len(lines) - 20 - (11 if i in amended else 0)
        for section in fill:
            section[1].append(' '.join(rng.sample(_FILLER, 2)))
        while len(render()) < rest - 4:
            rng.choice(fill)[1].append(' '.join(rng.sample(_FILLER, rng.randint(1, 3))))
        lines += render()
        if i in hyphenated:
            # Break the longest word of the governing law across two lines.
            word = max(record['governing_law'].split(), key=len)
            n = next(n for n in range(len(lines) - 1, 0, -1) if word in lines[n].split(' ')
                     or word + '.' in lines[n].split(' '))
            at = lines[n].index(word) + len(word) // 2
            lines[n:n + 1] = [lines[n][:at] + '-', lines[n][at:]]
        blocks = [[f'For and on behalf of the {sup_role}', '', 'Signature: ____________________',
                   f'Name: {sup_sign}', f'Title: {rng.choice(_TITLES)}',
                   f'Date: {_date_text(signed, date_style)}', ''],
                  [f'For and on behalf of the {cust_role}', '', 'Signature: ____________________',
                   f'Name: {cust_sign}', f'Title: {rng.choice(_TITLES)}',
                   f'Date: {_date_text(signed, date_style)}', '']]
        if rng.random() < 0.5:
            blocks.reverse()
        lines += ['SIGNATURES', '', 'Signed by the duly authorised representatives of the '
                  'parties.', ''] + blocks[0] + blocks[1]
        if i in amended:
            when = effective + timedelta(rng.randint(60, 300))
            if i in term_amended:
                term = rng.choice([t for t in (24, 36, 48, 60, 72) if t != record['term_months']])
                change = f'The initial term in the Term clause is replaced by {_number(term, "months")}.'
                record['term_months'] = term
            elif i in fee_amended:
                fee = rng.randint(24, 3600) * 50000
                change = (f'The annual fee in the Fees clause is replaced by {_money(fee, code)}, '
                          f'exclusive of applicable taxes.')
                record['annual_fee_cents'] = fee
            else:
                change = (f'The address of the {cust_role} for notices is changed to '
                          f'{rng.randint(1, 250)} Riverside Walk.')
            lines += ['-' * 40, '', 'AMENDMENT No. 1', '']
            lines += _wrapped(f'This Amendment No. 1 to Agreement No. {reference} takes effect '
                              f'on {_date_text(when, date_style)}. The parties agree as follows:')
            lines += [''] + _wrapped('1. ' + change)
            lines += [''] + _wrapped('2. All other terms of the Agreement remain unchanged.')
        name = f'contracts/c{i + 1:02d}.txt'
        files[name] = ('\n'.join(lines) + '\n').encode()
        truth[name] = record
    return files, {'records': truth, 'contradicted': sorted(contradicted),
                   'term_amended': sorted(term_amended), 'fee_amended': sorted(fee_amended),
                   'idle_amended': sorted(idle_amended), 'hyphenated': sorted(hyphenated)}
