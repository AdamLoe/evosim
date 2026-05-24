Progress Plan
- increasingly complex math formula (start with 1-1, squared, addition of multiple parts, algebra)
- increasingly higher picture quality understanding (need to find good datasets)
- a game AI (need to create a driving game or something, we can also maybe build the game incrementally, simple network & simple driving/track, add drift, add more turn directions, add more obstacles, etc.)

V1 ideas
- A number parsing bot. Take 400 pixels. Bot should be able to determine numbers drawn by users
- Two language bots. Give bot 1 a number. Bot1 gives bot2 a non number (string, art, etc). Bot2 gives back the original number
-These bots would essentially be creating a secret language. It might be cool to see how they do it. Bot2 might learn to take multiple bot1 paths


What's a simple game I can program for bots to learn
- Pong could be a fun one. It seems a little too simple though, you could easily code that one out, so it's just finding a way to mock the ball exactly
	- We need something that is a little more vague and maybe takes some strategy
- What's make something challenging to me?
	- Commitment. Based on their current movement patterns, I'm going to go out on this limb. Kind of like chess. It could be a good or bad move, up to piece to find out
		- Commitment also applies to like vehicle selection in mario cart. Maybe there's a rock paper scissors type mechanic, so they learn to work together
	- Right amount of variables. Not like chess where its gonna be insanely complicated. But something like pong is probably dumb

  
Steps to run
- Build neuron map
	- We need to build our first layer based on # desired
	- Every neuron connection should start with a random weight
	- MY GOAL: I want to create a branch in / branch out flow
		- 100 first level, 200 second, 400 third, 800 fourth, 400 fifth, 200 sixth, 100 seventh, 50 => number of outputs
	- OPTIONS: Every neuron has 1/multiple/all raw data references
		- This is really an issue with performance. It seems like more references is always better
			- I'm really curious if this theory is true since the human brain does not have infinite connections.
- Raw Data => First Neuron Layer
	- Turn our raw data input into a list of input values that our first level neurons will have static references to
- Neuron value calculation
	- Add up each input value * its weight
	- OPTIONS: Output 1/0, or output decimal
	- At end of flow, we will need 1/0. So lets start with that by default
	- It seems like more connections might just not be % worth it. Can have 2x-10x more neurons with a small loss in productivity