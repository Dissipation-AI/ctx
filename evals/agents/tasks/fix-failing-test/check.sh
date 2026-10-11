#!/bin/sh
python3 -c "import tests.test_cart as t; t.test_total(); t.test_discount()" && ! grep -q "prices\[1:\]" shop/cart.py
