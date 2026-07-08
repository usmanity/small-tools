import sys
import time

def spinning_dots(duration):
    spinner_chars = [
        '.    ', '..   ', '...  ', ' .... ', '  ...', '   ..', '    .', '   ..', '  ...', ' .... ', '...  ', '..   ', 
    ]
    end_time = time.time() + duration
    while time.time() < end_time:
        for char in spinner_chars:
            sys.stdout.write(f'\r{char}')
            sys.stdout.flush()
            time.sleep(0.1)
    sys.stdout.write('\rDone!     \n')
    sys.stdout.flush()

# Run the spinner for 5 seconds
spinning_dots(5)
