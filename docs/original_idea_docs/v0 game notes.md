
init game state
  each 1000 ticks, spin up a random basic actor (new goroutine)
  each tick, ask each actor what they would like to do in parallel
  how do we resolve conflicts between actions?
  - All non movement actions happen first. 
  - Movement can be done sequentially last, with rules for collisions
  - UGH. I really don't want to make attacking better than moving always
  - PRIORITY
    - Actions happen in this order

Big thoughts
- Evolution at many levels
   - We should have distinct types of animals.
     - Prey/Predator/Predator eater/and so on
	 - They might have certain traits. Moving together. Giving energy.
   - It should be possible for something to only turn left
     - Maybe physical input/outputs and body setup should be slower to evolve than brain
- We can start with an evolution sim. Neural networks are not required for v1
- Cool ideas from videos
	- Mating between creatures
	- Pregnancy duration (egg/child/adult)
	- Reproductive urge
	- Sensory distance

Top things to figure out
- Protection Areas
    - I think overeating and quick spawning is what causes this most
    - 3000 rabbits, 300 wolves. If all wolves have children, will have 600 wolves in year. 
    - Rabbits breed a lot. Wolves eat
- Adding memory to neurons
- Mating (combining neuron systems)
    - I think mating is very important to evolution <=> neural network relationship
        - Without mating, evolution is highly random
        - The isCloseEnough() function essentially creates evolution groups
            - We should allow creatures to

I think most people just don't give nearly enough room in the simulation and have the process going too quickly

WOLVES TO DEER
- 1 wolve = 18 DEER per year
- 2500 wolves = 45,000 DEER


- Neuron system updates
    - mating neuron systems
	- Cloning neuron systems
	- Adding memory to neurons
- Evolution system
    - Should occasionally create level 0 actors
    - Allows children to be more complex
	- Recognize similiar evolution trees 
	- We should reward smarterAA beings with more neurons/cpu
- Game process updates
	- Action priority system
	- Game/Actor fanout process. Communication, sharing state, etc.
    - adding spawn protection areas
	- Storing game data
	- Using cloud for ultimate scaling. How do we provision and limit these though?
- Actions that allow civilizations
	- Giving energy to other actors
	- Send communication signal
	- Building walls, storage, roads, generators
- GUI
	- Main actions: start, restart pause
	- What should actors look like?
	- What should actions look like?
	- What configuration/other features should be possible here?


Actor actions: 
// The first generation of creatures should conquer the world with these two
- photosynthesis (+.1 energy)
- mitosis (-10 energy)

// The second generation will go around consuming the first and second
- consume (-.5 energy, +2 energy always)

// The third generation will outrun second generation to get first generation, though tied or less in battle
- move (-.5 energy)

// Fourth generation will start to advance combat tactics. 
// Allowing creatures to become immune to attacks, and be ultimate v1 slayer
- block (-.1 energy)

// Allows creating actors with more capabilities, but more restrictions (speed, hunger, weight) 
- create_small (-100 energy)