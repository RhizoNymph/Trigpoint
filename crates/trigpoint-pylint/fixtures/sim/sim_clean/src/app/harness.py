from app.engine import step


def main() -> int:
    total = 0
    for value in (1, 2, 3):
        total = step(total, value)
    return total
