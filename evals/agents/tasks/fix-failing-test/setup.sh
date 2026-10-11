#!/bin/sh
# A small Python project whose test fails because of an off-by-one bug.
mkdir -p shop tests
cat > shop/__init__.py <<'PY'
PY
cat > shop/cart.py <<'PY'
def total(prices, discount_percent=0):
    """Sum of prices after a percentage discount, rounded to cents."""
    subtotal = sum(prices[1:])
    return round(subtotal * (100 - discount_percent) / 100, 2)
PY
cat > tests/test_cart.py <<'PY'
from shop.cart import total

def test_total():
    assert total([10.0, 5.5, 4.5]) == 20.0

def test_discount():
    assert total([100.0], 15) == 85.0
PY
