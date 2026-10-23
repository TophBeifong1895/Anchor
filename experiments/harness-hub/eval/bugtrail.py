"""The `shop` package of the bugtrail task: about 30 modules, 7 symptom
reports, 4 root causes in shared modules away from the symptoms. Hidden
tests check each report on its own path and on another caller of the same
root cause, so that a patch at the symptom earns half of what a fix at the
cause earns; regression groups check what a careless fix would break.
"""

import random

# ---------------------------------------------------------------------------
# The four modules holding a root cause, and their fixes
# ---------------------------------------------------------------------------

CACHE = '''"""A process-wide memoizing cache."""

import functools

_CACHE = {}
_STATS = {'hits': 0, 'misses': 0}


def memoize(fn):
    """Cache fn's results per function and per full set of arguments,
    positional and keyword: two calls share a result only when they pass
    equal arguments in the same way."""
    @functools.wraps(fn)
    def wrapper(*args, **kwargs):
        key = (fn.__module__, fn.__qualname__, args)
        if key in _CACHE:
            _STATS['hits'] += 1
            return _CACHE[key]
        _STATS['misses'] += 1
        value = fn(*args, **kwargs)
        _CACHE[key] = value
        return value
    return wrapper


def clear():
    """Forget every cached result."""
    _CACHE.clear()


def stats():
    """Hits and misses since the process started."""
    return dict(_STATS)
'''

EVENTS = '''"""Events: the records the shop keeps of what happened."""

import itertools

_SEQUENCE = itertools.count(1)


def make_event(kind, payload, tags=[]):
    """A new event {id, kind, payload, tags}. Its tags are the given tags,
    in order, followed by kind. The list passed in is left as it is, and no
    two events share a tags list."""
    tags.append(kind)
    return {'id': next(_SEQUENCE), 'kind': kind, 'payload': dict(payload), 'tags': tags}


def tagged(events, tag):
    """The events carrying tag, in order."""
    return [event for event in events if tag in event['tags']]


def kinds(events):
    """The distinct kinds of events, sorted."""
    return sorted({event['kind'] for event in events})
'''

MONEY = '''"""Money helpers. Amounts are integer cents."""


def round_cents(value):
    """Round an amount in cents to a whole cent, halves away from zero:
    102.5 is 103 and -102.5 is -103."""
    return int(round(value))


def format_cents(cents, symbol='$'):
    """Format cents as an amount: format_cents(-123456) is '-$1,234.56'."""
    sign = '-' if cents < 0 else ''
    return f'{sign}{symbol}{abs(cents) // 100:,}.{abs(cents) % 100:02d}'


def split_evenly(cents, parts):
    """Split cents into parts shares that differ by at most one cent and sum
    to cents, larger shares first."""
    share, rest = divmod(cents, parts)
    return [share + 1] * rest + [share] * (parts - rest)


def percent_of(cents, pct):
    """pct percent of cents, as an exact float."""
    return cents * pct / 100
'''

REGISTRY = '''"""The registry of payment methods. Each module of shop.payments registers
its method when it is imported."""

_METHODS = {}


def register(name, method):
    """Register method under name. A name may be registered only once; a
    second registration raises ValueError."""
    if name in _METHODS:
        raise ValueError(f'payment method {name!r} is already registered')
    _METHODS[name] = method


def get(name):
    """The payment method registered under name. Every method of the modules
    of shop.payments is available, whatever the caller imported before. An
    unknown name raises KeyError."""
    if name not in _METHODS:
        raise KeyError(f'unknown payment method {name!r}')
    return _METHODS[name]


def names():
    """The names of all payment methods of shop.payments, sorted."""
    return sorted(_METHODS)
'''

FIXED_ROOTS = {
    'shop/util/cache.py': CACHE.replace(
        'key = (fn.__module__, fn.__qualname__, args)',
        'key = (fn.__module__, fn.__qualname__, args, tuple(sorted(kwargs.items())))'),
    'shop/util/events.py': EVENTS.replace(
        'def make_event(kind, payload, tags=[]):', 'def make_event(kind, payload, tags=None):'
    ).replace('    tags.append(kind)\n', '    tags = list(tags or []) + [kind]\n'),
    'shop/util/money.py': MONEY.replace(
        '"""Money helpers. Amounts are integer cents."""\n',
        '"""Money helpers. Amounts are integer cents."""\n\nimport math\n'
    ).replace('    return int(round(value))\n',
              '    whole = math.floor(abs(value) + 0.5)\n'
              '    return int(whole if value >= 0 else -whole)\n'),
    'shop/registry.py': REGISTRY.replace(
        '"""\n\n_METHODS = {}\n',
        '"""\n\nimport importlib\nimport pkgutil\n\n_METHODS = {}\n\n\n'
        'def _load():\n'
        '    """Import every module of shop.payments, so that each registers."""\n'
        '    import shop.payments\n'
        '    for module in pkgutil.iter_modules(shop.payments.__path__):\n'
        '        importlib.import_module(f\'shop.payments.{module.name}\')\n'
    ).replace("    if name not in _METHODS:\n        raise KeyError",
              "    _load()\n    if name not in _METHODS:\n        raise KeyError"
              ).replace('    return sorted(_METHODS)\n', '    _load()\n    return sorted(_METHODS)\n'),
}

# ---------------------------------------------------------------------------
# The callers
# ---------------------------------------------------------------------------

CORE = {
    'shop/__init__.py': '"""A small shop backend: catalog, pricing, orders, payments, reports."""\n',
    'shop/util/__init__.py': '"""Helpers shared across the shop."""\n',
    'shop/util/cache.py': CACHE,
    'shop/util/events.py': EVENTS,
    'shop/util/money.py': MONEY,
    'shop/registry.py': REGISTRY,
    'shop/payments/__init__.py': '"""Payment methods. Each module registers one method."""\n',
    'shop/catalog.py': '''"""The product catalog."""

PRODUCTS = {
    'SKU-100': {'name': 'Desk lamp', 'price': 1999, 'grams': 1200, 'category': 'home'},
    'SKU-101': {'name': 'Notebook', 'price': 450, 'grams': 300, 'category': 'office'},
    'SKU-102': {'name': 'Fountain pen', 'price': 3475, 'grams': 40, 'category': 'office'},
    'SKU-103': {'name': 'Kettle', 'price': 2850, 'grams': 1500, 'category': 'kitchen'},
    'SKU-104': {'name': 'Tea tin', 'price': 725, 'grams': 250, 'category': 'kitchen'},
    'SKU-105': {'name': 'Cushion', 'price': 1600, 'grams': 600, 'category': 'home'},
    'SKU-106': {'name': 'Stapler', 'price': 980, 'grams': 350, 'category': 'office'},
    'SKU-107': {'name': 'Mug', 'price': 600, 'grams': 400, 'category': 'kitchen'},
}


def base_price(sku):
    """The list price of sku in US cents. An unknown sku raises KeyError."""
    return PRODUCTS[sku]['price']


def in_category(category):
    """The skus of category, sorted."""
    return sorted(sku for sku, item in PRODUCTS.items() if item['category'] == category)


def weight(sku, quantity=1):
    """The shipping weight in grams of quantity items of sku."""
    return PRODUCTS[sku]['grams'] * quantity
''',
    'shop/pricing.py': '''"""Prices of catalog items in several currencies."""

from shop.catalog import base_price
from shop.util.cache import memoize
from shop.util.money import round_cents

RATES = {'USD': 1.0, 'EUR': 0.92, 'GBP': 0.79, 'CHF': 0.88}


@memoize
def quote(sku, currency='USD'):
    """The price of sku in currency, in cents of that currency."""
    return round_cents(base_price(sku) * RATES[currency])


def quote_all(sku):
    """The price of sku in every currency, as {currency: cents}."""
    return {currency: quote(sku, currency=currency) for currency in sorted(RATES)}
''',
    'shop/shipping.py': '''"""Shipping costs."""

from shop.catalog import weight
from shop.util.cache import memoize

ZONES = {'domestic': 499, 'eu': 1299, 'world': 2499}
HEAVY_GRAMS = 5000


@memoize
def rate(zone, express=False):
    """The shipping cost in cents of a parcel to zone. Express costs twice
    the standard rate plus 500."""
    base = ZONES[zone]
    return base * 2 + 500 if express else base


def parcel_cost(zone, lines, express=False):
    """The cost of shipping lines [(sku, quantity)]: the zone rate, plus 800
    for a parcel heavier than HEAVY_GRAMS."""
    grams = sum(weight(sku, quantity) for sku, quantity in lines)
    return rate(zone, express=express) + (800 if grams > HEAVY_GRAMS else 0)
''',
    'shop/inventory.py': '''"""Stock levels per warehouse."""

from shop.util.cache import memoize

STOCK = {
    ('main', 'SKU-100'): 12, ('north', 'SKU-100'): 3, ('main', 'SKU-101'): 140,
    ('north', 'SKU-101'): 0, ('main', 'SKU-102'): 7, ('south', 'SKU-102'): 25,
    ('main', 'SKU-103'): 0, ('south', 'SKU-103'): 4,
}


@memoize
def available(sku, warehouse='main'):
    """Units of sku in stock at warehouse; 0 when it holds none."""
    return STOCK.get((warehouse, sku), 0)


def total_available(sku):
    """Units of sku in stock over all warehouses."""
    return sum(units for (_, item), units in STOCK.items() if item == sku)
''',
    'shop/loyalty.py': '''"""Loyalty points."""

from shop.util.cache import memoize

MULTIPLIER = {'basic': 1, 'silver': 1.5, 'gold': 2}


@memoize
def points(cents, tier='basic'):
    """Points earned on a purchase of cents: one per full 100 cents, times
    the tier's multiplier, rounded down."""
    return int(cents // 100 * MULTIPLIER[tier])


def tier_for(yearly_cents):
    """The tier earned by a year's purchases."""
    if yearly_cents >= 500000:
        return 'gold'
    return 'silver' if yearly_cents >= 100000 else 'basic'
''',
    'shop/orders.py': '''"""The audit trail of orders."""

from shop.util.events import make_event

_LOG = []


def record(order_id, action):
    """Record action on an order and return its event, whose tags are
    ['order.<action>']."""
    event = make_event('order.' + action, {'order': order_id})
    _LOG.append(event)
    return event


def history(order_id):
    """The events of order_id, oldest first."""
    return [event for event in _LOG if event['payload']['order'] == order_id]
''',
    'shop/notifications.py': '''"""Notifications to users."""

from shop.util.events import make_event

_SENT = []


def notify(user, message, urgent=False):
    """Send message to user and return its event, whose tags are
    ['notification'], or ['urgent', 'notification'] when urgent."""
    if urgent:
        event = make_event('notification', {'user': user, 'message': message}, ['urgent'])
    else:
        event = make_event('notification', {'user': user, 'message': message})
    _SENT.append(event)
    return event


def sent_to(user):
    """The notifications sent to user, oldest first."""
    return [event for event in _SENT if event['payload']['user'] == user]
''',
    'shop/reviews.py': '''"""Product reviews."""

from shop.util.events import make_event

_REVIEWS = []


def submit(sku, stars, text):
    """Store a review of 1 to 5 stars and return its event, whose tags are
    ['review']."""
    if not 1 <= stars <= 5:
        raise ValueError('stars must be 1 to 5')
    event = make_event('review', {'sku': sku, 'stars': stars, 'text': text})
    _REVIEWS.append(event)
    return event


def average_stars(sku):
    """The mean stars of sku's reviews, or None without reviews."""
    stars = [e['payload']['stars'] for e in _REVIEWS if e['payload']['sku'] == sku]
    return sum(stars) / len(stars) if stars else None
''',
    'shop/webhooks.py': '''"""Outgoing webhooks."""

from shop.util.events import make_event

_QUEUE = []


def dispatch(url, kind, body):
    """Queue a webhook call and return its event, whose tags are
    ['webhook.<kind>']."""
    event = make_event('webhook.' + kind, {'url': url, 'body': body})
    _QUEUE.append(event)
    return event


def pending():
    """The queued webhook events, oldest first."""
    return list(_QUEUE)
''',
    'shop/tax.py': '''"""Sales tax."""

from shop.util.money import round_cents


def tax(cents, rate_pct):
    """The tax on cents at rate_pct percent, in whole cents."""
    return round_cents(cents * rate_pct / 100)


def with_tax(cents, rate_pct):
    """cents plus its tax."""
    return cents + tax(cents, rate_pct)
''',
    'shop/discounts.py': '''"""Discounts."""

from shop.util.money import round_cents


def apply(cents, pct):
    """cents less a discount of pct percent, the discount in whole cents."""
    return cents - round_cents(cents * pct / 100)


def best(cents, offers):
    """The lowest price among the percentage offers, or cents without any."""
    return min([cents] + [apply(cents, pct) for pct in offers])
''',
    'shop/currency.py': '''"""Currency conversion of amounts in cents."""

from shop.util.money import round_cents


def convert(cents, rate):
    """cents converted at rate, in whole cents of the target currency."""
    return round_cents(cents * rate)
''',
    'shop/reports/__init__.py': '"""Reports over orders and stock."""\n',
    'shop/reports/daily.py': '''"""The daily sales report."""

from shop.util.money import format_cents, round_cents


def average_order(totals):
    """The mean of the order totals in whole cents, or 0 without orders."""
    return round_cents(sum(totals) / len(totals)) if totals else 0


def summary(totals):
    """A one-line summary of a day's order totals."""
    return f'{len(totals)} orders, {format_cents(sum(totals))}, ' \\
        f'average {format_cents(average_order(totals))}'
''',
    'shop/payments/card.py': '''"""Card payments: a 2 percent fee, charged to the shop."""

from shop import registry


class CardMethod:
    name = 'card'

    def charge(self, order):
        return {'method': 'card', 'order': order['id'], 'amount': order['total'],
                'fee': order['total'] * 2 // 100}

    def refund(self, order, cents):
        return {'method': 'card', 'order': order['id'], 'amount': -cents}


registry.register('card', CardMethod())
''',
    'shop/payments/voucher.py': '''"""Gift voucher payments: no fee."""

from shop import registry


class VoucherMethod:
    name = 'voucher'

    def charge(self, order):
        return {'method': 'voucher', 'order': order['id'], 'amount': order['total'], 'fee': 0}

    def refund(self, order, cents):
        return {'method': 'voucher', 'order': order['id'], 'amount': -cents, 'reissued': True}


registry.register('voucher', VoucherMethod())
''',
    'shop/payments/invoice.py': '''"""Payment by invoice: a flat fee of 150 cents."""

from shop import registry


class InvoiceMethod:
    name = 'invoice'

    def charge(self, order):
        return {'method': 'invoice', 'order': order['id'], 'amount': order['total'], 'fee': 150}

    def refund(self, order, cents):
        return {'method': 'invoice', 'order': order['id'], 'amount': -cents, 'credit_note': True}


registry.register('invoice', InvoiceMethod())
''',
    'shop/checkout.py': '''"""Checkout: total an order and take payment."""

from shop import registry
from shop.payments import card  # noqa: F401  the default method
from shop.tax import tax


def total(lines, rate_pct):
    """The total of lines [(unit_cents, quantity)] with tax at rate_pct."""
    net = sum(unit * quantity for unit, quantity in lines)
    return net + tax(net, rate_pct)


def pay(order, method='card'):
    """Charge order {id, total} with the payment method named method."""
    return registry.get(method).charge(order)
''',
    'shop/refunds.py': '''"""Refunds."""

from shop import registry


def refund(order, method, cents):
    """Refund cents of order through the payment method named method."""
    if cents <= 0 or cents > order['total']:
        raise ValueError('refund must be above 0 and at most the order total')
    return registry.get(method).refund(order, cents)
''',
}

# ---------------------------------------------------------------------------
# Modules around them: plain, correct code, so that the package is large
# ---------------------------------------------------------------------------

_ENTITIES = {
    'shop/customers.py': ('customer', ['id', 'name', 'email', 'country', 'spent']),
    'shop/search.py': ('product', ['sku', 'name', 'category', 'price', 'rating']),
    'shop/validation.py': ('address', ['street', 'city', 'postcode', 'country', 'phone']),
    'shop/export.py': ('row', ['order', 'sku', 'quantity', 'unit', 'status']),
    'shop/reports/weekly.py': ('day', ['date', 'orders', 'revenue', 'refunds', 'visits']),
    'shop/reports/stock.py': ('level', ['sku', 'warehouse', 'units', 'reorder', 'supplier']),
    'shop/util/text.py': ('line', ['text', 'width', 'indent', 'lang', 'source']),
    'shop/util/dates.py': ('span', ['start', 'end', 'label', 'zone', 'owner']),
    'shop/suppliers.py': ('supplier', ['id', 'name', 'country', 'lead_days', 'rating']),
    'shop/returns.py': ('return_', ['order', 'sku', 'reason', 'days', 'condition']),
    'shop/warehouses.py': ('site', ['code', 'city', 'manager', 'capacity', 'staff']),
    'shop/campaigns.py': ('campaign', ['code', 'channel', 'owner', 'budget', 'clicks']),
}


def _filler(path, entity, fields, rng):
    title = path.rsplit('/', 1)[1][:-3].replace('_', ' ')
    out = [f'"""Helpers over {title} records: dicts with the keys '
           f'{", ".join(fields)}."""', '', '']
    numeric, textual = fields[3:], fields[:3]
    for field in fields:
        out += [f'def by_{field}({entity}s):',
                f'    """The {entity}s grouped by {field}, as {{value: [records]}}, in input',
                '    order within each group."""',
                '    groups = {}',
                f'    for record in {entity}s:',
                f'        groups.setdefault(record[{field!r}], []).append(record)',
                '    return groups', '', '']
        out += [f'def sorted_by_{field}({entity}s, reverse=False):',
                f'    """The {entity}s sorted by {field}; equal values keep their order."""',
                f'    return sorted({entity}s, key=lambda record: record[{field!r}], '
                'reverse=reverse)', '', '']
        out += [f'def distinct_{field}({entity}s):',
                f'    """The distinct values of {field}, sorted."""',
                f'    return sorted({{record[{field!r}] for record in {entity}s}})', '', '']
        out += [f'def count_by_{field}({entity}s):',
                f'    """How many {entity}s have each value of {field}, as {{value: count}}."""',
                '    counts = {}',
                f'    for record in {entity}s:',
                f'        counts[record[{field!r}]] = counts.get(record[{field!r}], 0) + 1',
                '    return counts', '', '']
        out += [f'def index_by_{field}({entity}s):',
                f'    """The first {entity} of each value of {field}, as {{value: record}}."""',
                '    index = {}',
                f'    for record in {entity}s:',
                f'        index.setdefault(record[{field!r}], record)',
                '    return index', '', '']
    for field in numeric:
        limit = rng.randint(2, 50)
        out += [f'def total_{field}({entity}s):',
                f'    """The sum of {field} over the {entity}s."""',
                f'    return sum(record[{field!r}] for record in {entity}s)', '', '']
        out += [f'def top_{field}({entity}s, n={limit}):',
                f'    """The n {entity}s with the largest {field}, largest first."""',
                f'    return sorted({entity}s, key=lambda record: -record[{field!r}])[:n]', '', '']
        out += [f'def mean_{field}({entity}s):',
                f'    """The mean {field}, or None for no {entity}s."""',
                f'    if not {entity}s:',
                '        return None',
                f'    return total_{field}({entity}s) / len({entity}s)', '', '']
        out += [f'def above_{field}({entity}s, threshold):',
                f'    """The {entity}s whose {field} is above threshold."""',
                f'    return [record for record in {entity}s if record[{field!r}] > threshold]',
                '', '']
    for field in textual:
        out += [f'def find_{field}({entity}s, needle):',
                f'    """The {entity}s whose {field} contains needle, ignoring case."""',
                '    needle = str(needle).lower()',
                f'    return [record for record in {entity}s '
                f'if needle in str(record[{field!r}]).lower()]', '', '']
    out += [f'def missing({entity}s):',
            f'    """For each {entity}, the keys it lacks or leaves empty, as',
            '    [(index, [keys])], only for records lacking any."""',
            '    result = []',
            f'    for index, record in enumerate({entity}s):',
            f'        absent = [key for key in {fields!r} if record.get(key) in (None, \'\')]',
            '        if absent:',
            '            result.append((index, absent))',
            '    return result', '']
    return '\n'.join(out) + '\n'


def package():
    rng = random.Random(20261021 + 33)
    files = dict(CORE)
    for path, (entity, fields) in _ENTITIES.items():
        files[path] = _filler(path, entity, fields, rng)
    return dict(sorted(files.items()))


VISIBLE = '''"""Tests of the shop. Run from the directory holding shop/:
python3.12 test_shop.py"""

from shop.catalog import base_price, in_category
from shop.checkout import pay, total
from shop.orders import history, record
from shop.pricing import quote
from shop.shipping import rate
from shop.tax import tax
from shop.util.money import format_cents, split_evenly

assert base_price('SKU-101') == 450
assert in_category('kitchen') == ['SKU-103', 'SKU-104', 'SKU-107']
assert quote('SKU-100') == 1999
assert rate('domestic') == 499
assert tax(1000, 20) == 200
assert total([(1000, 2)], 10) == 2200
assert pay({'id': 'O1', 'total': 5000})['fee'] == 100
assert record('O9', 'created')['kind'] == 'order.created'
assert [e['kind'] for e in history('O9')] == ['order.created']
assert format_cents(-123456) == '-$1,234.56'
assert split_evenly(10, 3) == [4, 3, 3]
print('OK')
'''

# Each issue: (title, root cause, the report's code, what it printed, what
# was expected, the hidden test on the reported path, the hidden test on
# another caller of the same cause).
ISSUES = [
    ('A quote in EUR comes back in dollars', 'cache',
     "from shop.pricing import quote; print(quote('SKU-100'), quote('SKU-100', currency='EUR'))",
     '1999 1999', '1999 1839',
     "from shop.pricing import quote\nassert quote('SKU-100') == 1999\n"
     "assert quote('SKU-100', currency='EUR') == 1839\n"
     "assert quote('SKU-102', currency='GBP') == 2745\nassert quote('SKU-102') == 3475\n",
     "from shop.inventory import available\nassert available('SKU-100') == 12\n"
     "assert available('SKU-100', warehouse='north') == 3\n"
     "assert available('SKU-102', warehouse='south') == 25\n"),
    ('Express shipping costs the same as standard', 'cache',
     "from shop.shipping import rate; print(rate('eu'), rate('eu', express=True))",
     '1299 1299', '1299 3098',
     "from shop.shipping import rate\nassert rate('eu') == 1299\n"
     "assert rate('eu', express=True) == 3098\nassert rate('world', express=True) == 5498\n",
     "from shop.loyalty import points\nassert points(25000) == 250\n"
     "assert points(25000, tier='gold') == 500\nassert points(25000, tier='silver') == 375\n"),
    ('Order history shows actions of other orders in its tags', 'events',
     "from shop.orders import record; record('O1', 'created'); "
     "print(record('O2', 'paid')['tags'])",
     "['order.created', 'order.paid']", "['order.paid']",
     "from shop.orders import record\nrecord('O1', 'created')\n"
     "assert record('O2', 'paid')['tags'] == ['order.paid']\n"
     "assert record('O1', 'shipped')['tags'] == ['order.shipped']\n",
     "from shop.reviews import submit\nsubmit('SKU-100', 5, 'bright')\n"
     "assert submit('SKU-101', 3, 'thin paper')['tags'] == ['review']\n"),
    ('Every notification is tagged notification several times', 'events',
     "from shop.notifications import notify; notify('ana', 'hi'); "
     "print(notify('ben', 'hello')['tags'])",
     "['notification', 'notification']", "['notification']",
     "from shop.notifications import notify\nnotify('ana', 'hi')\n"
     "assert notify('ben', 'hello')['tags'] == ['notification']\n"
     "assert notify('ana', 'now', urgent=True)['tags'] == ['urgent', 'notification']\n",
     "from shop.webhooks import dispatch\ndispatch('https://a.test', 'paid', {})\n"
     "assert dispatch('https://b.test', 'shipped', {})['tags'] == ['webhook.shipped']\n"),
    ('Tax is one cent short on some orders', 'money',
     "from shop.tax import tax; print(tax(1025, 10), tax(4250, 5))",
     '102 212', '103 213',
     "from shop.tax import tax, with_tax\nassert tax(1025, 10) == 103\n"
     "assert tax(4250, 5) == 213\nassert with_tax(1025, 10) == 1128\n",
     "from shop.reports.daily import average_order\n"
     "assert average_order([100, 105]) == 103\nassert average_order([1, 2, 3, 4]) == 3\n"),
    ('A 15 percent discount takes a cent too little', 'money',
     "from shop.discounts import apply; print(apply(30, 15), apply(4250, 1))",
     '26 4208', '25 4207',
     "from shop.discounts import apply, best\nassert apply(30, 15) == 25\n"
     "assert apply(4250, 1) == 4207\nassert best(30, [10, 15]) == 25\n",
     "from shop.currency import convert\nassert convert(5, 0.5) == 3\n"
     "assert convert(1300, 0.025) == 33\n"),
    ('Paying with a voucher fails with unknown payment method', 'registry',
     "from shop.checkout import pay; print(pay({'id': 'O7', 'total': 5000}, 'voucher'))",
     "KeyError: \"unknown payment method 'voucher'\"",
     "{'method': 'voucher', 'order': 'O7', 'amount': 5000, 'fee': 0}",
     "from shop.checkout import pay\n"
     "assert pay({'id': 'O7', 'total': 5000}, 'voucher')['method'] == 'voucher'\n"
     "assert pay({'id': 'O8', 'total': 900})['method'] == 'card'\n",
     "from shop.refunds import refund\n"
     "assert refund({'id': 'O9', 'total': 5000}, 'invoice', 1000)['credit_note'] is True\n"),
]

REGRESSIONS = {
    'regression_cache': (
        "from shop.util.cache import memoize\ncalls = []\n\n@memoize\ndef f(a, b=0):\n"
        "    calls.append((a, b))\n    return a + b\n\n@memoize\ndef g(a):\n"
        "    calls.append(('g', a))\n    return -a\n\n"
        "assert [f(1), f(1), f(2), f(3, b=1), f(3, b=1), g(1), g(1)] == [1, 1, 2, 4, 4, -1, -1]\n"
        "assert calls == [(1, 0), (2, 0), (3, 1), ('g', 1)], calls\n"),
    'regression_events': (
        "from shop.util.events import make_event, tagged\n"
        "e = make_event('sale', {'n': 1}, ['eu', 'web'])\n"
        "assert e['tags'] == ['eu', 'web', 'sale'] and e['payload'] == {'n': 1}\n"
        "f = make_event('sale', {'n': 2}, ['pos'])\nassert f['tags'] == ['pos', 'sale']\n"
        "assert f['id'] > e['id']\nassert tagged([e, f], 'web') == [e]\n"),
    'regression_money': (
        "from shop.util.money import round_cents\n"
        "assert [round_cents(v) for v in (-103.5, -1.5, -0.4, 7.4, 7.6, 0.0, -7.6)] == "
        "[-104, -2, 0, 7, 8, 0, -8]\n"
        "assert all(type(round_cents(v)) is int for v in (1.5, -2.5, 3.0))\n"),
    'regression_registry': (
        "from shop import registry\nfrom shop.checkout import pay\n"
        "try:\n    registry.get('bitcoin')\n    raise SystemExit('unknown name accepted')\n"
        "except KeyError:\n    pass\n"
        "registry.register('cash', object())\n"
        "try:\n    registry.register('cash', object())\n    raise SystemExit('registered twice')\n"
        "except ValueError:\n    pass\n"
        "assert 'card' in registry.names() and 'cash' in registry.names()\n"
        "assert pay({'id': 'O1', 'total': 100})['method'] == 'card'\n"),
}


def hidden_tests():
    tests = {}
    for n, issue in enumerate(ISSUES, start=1):
        tests[f'issue{n}'] = issue[5] + "print('OK')\n"
        tests[f'issue{n}_other_caller'] = issue[6] + "print('OK')\n"
    for name, code in REGRESSIONS.items():
        tests[name] = code + "print('OK')\n"
    return tests


def issues_md():
    out = ['# Open issues', '',
           'Each report comes from a user; the code shown was run with python3.12 in the '
           'directory that holds shop/, in a fresh interpreter.', '']
    for n, (title, _, code, got, want, _, _) in enumerate(ISSUES, start=1):
        out += [f'## {n}. {title}', '', '```', f'python3.12 -c "{code}"', '```', '',
                f'Printed: `{got}`', '', f'Expected: `{want}`', '']
    return '\n'.join(out)
